"""Background poller: periodically fetches every source, regroups markets and
re-runs arbitrage detection, publishing results into the shared Store."""
from __future__ import annotations

import asyncio
import logging

from .arbitrage import find_opportunities
from .config import Settings
from .matching import group_markets
from .models import MarketSnapshot, PollerStatus, utcnow
from .sources.base import OddsSource
from .store import Store
from .ws import Broadcaster

logger = logging.getLogger(__name__)


class Poller:
    def __init__(
        self,
        sources: list[OddsSource],
        store: Store,
        settings: Settings,
        broadcaster: Broadcaster | None = None,
    ) -> None:
        self._sources = sources
        self._store = store
        self._settings = settings
        self._broadcaster = broadcaster
        # opportunity id -> profit margin as of the last published message,
        # used to diff polls into new/changed/expired WebSocket events.
        self._last_published: dict[str, float] = {}
        self._task: asyncio.Task | None = None
        self._wake = asyncio.Event()
        self._started_sources = False
        self.status = PollerStatus(
            running=False,
            poll_interval_seconds=settings.poll_interval_seconds,
            sources=[s.name for s in sources],
        )

    async def start(self) -> None:
        if self._task is not None and not self._task.done():
            return
        if not self._started_sources:
            for source in self._sources:
                await source.start()
            self._started_sources = True
        self.status.running = True
        self._task = asyncio.create_task(self._run(), name="arb-poller")

    async def stop(self) -> None:
        self.status.running = False
        if self._task is not None:
            self._task.cancel()
            try:
                await self._task
            except asyncio.CancelledError:
                pass
            self._task = None

    async def shutdown(self) -> None:
        await self.stop()
        for source in self._sources:
            try:
                await source.stop()
            except Exception:  # noqa: BLE001 - best-effort cleanup
                logger.exception("Error stopping source %s", source.name)

    def trigger(self) -> None:
        """Request an immediate poll (used by POST /poll)."""
        self._wake.set()

    async def _run(self) -> None:
        while True:
            await self.poll_once()
            self._wake.clear()
            try:
                await asyncio.wait_for(
                    self._wake.wait(), timeout=self.status.poll_interval_seconds
                )
            except asyncio.TimeoutError:
                pass

    async def poll_once(self) -> None:
        started = utcnow()
        self.status.last_poll_started_at = started
        results = await asyncio.gather(
            *(source.fetch_markets() for source in self._sources),
            return_exceptions=True,
        )
        errors: list[str] = []
        all_markets: list[MarketSnapshot] = []
        for source, result in zip(self._sources, results):
            if isinstance(result, BaseException):
                logger.exception(
                    "Source %s failed", source.name, exc_info=result
                )
                errors.append(f"{source.name}: {result}")
                # Keep this source's previous markets rather than dropping them.
                all_markets.extend(
                    self._store.markets_for_feed(source.name)
                )
                continue
            self._store.replace_markets(source.name, result)
            all_markets.extend(result)

        groups = group_markets(
            all_markets,
            threshold=self._settings.event_match_threshold,
            max_start_diff_minutes=self._settings.event_match_max_start_diff_minutes,
        )
        found = find_opportunities(
            groups,
            commission=self._settings.betfair_commission,
            min_profit_margin=self._settings.min_profit_margin,
            outcome_threshold=self._settings.event_match_threshold,
        )
        self._store.record_opportunities(found)

        finished = utcnow()
        self.status.last_poll_finished_at = finished
        self.status.last_poll_duration_seconds = (
            finished - started
        ).total_seconds()
        self.status.polls_completed += 1
        self.status.last_error = "; ".join(errors) if errors else None
        markets, active = self._store.counts()
        self.status.markets_tracked = markets
        self.status.active_opportunities = active
        if found:
            logger.info(
                "Poll #%d: %d markets, %d opportunities (best %.2f%%)",
                self.status.polls_completed,
                markets,
                len(found),
                found[0].profit_margin * 100,
            )
        self._broadcast_poll_result()

    def _broadcast_poll_result(self) -> None:
        if self._broadcaster is None:
            return
        current = {o.id: o for o in self._store.opportunities(active_only=True)}
        new = [o for oid, o in current.items() if oid not in self._last_published]
        changed = [
            o
            for oid, o in current.items()
            if oid in self._last_published
            and o.profit_margin != self._last_published[oid]
        ]
        expired = sorted(set(self._last_published) - set(current))
        self._last_published = {
            oid: o.profit_margin for oid, o in current.items()
        }
        self._broadcaster.publish(
            {
                "type": "poll",
                "status": self.status.model_dump(mode="json"),
                "new": [o.model_dump(mode="json") for o in new],
                "changed": [o.model_dump(mode="json") for o in changed],
                "expired": expired,
            }
        )

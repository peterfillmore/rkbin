"""Thread-safe in-memory state shared between the poller and the API."""
from __future__ import annotations

import threading
from datetime import timedelta

from .models import MarketSnapshot, Opportunity, utcnow


class Store:
    def __init__(self, opportunity_ttl_seconds: float = 120.0) -> None:
        self._lock = threading.Lock()
        self._markets: dict[str, MarketSnapshot] = {}
        # feed name (OddsSource.name) -> keys of the markets it last produced
        self._feeds: dict[str, set[str]] = {}
        self._opportunities: dict[str, Opportunity] = {}
        self._ttl = timedelta(seconds=opportunity_ttl_seconds)

    # --- markets ---

    def replace_markets(self, feed: str, snapshots: list[MarketSnapshot]) -> None:
        """Replace all markets previously produced by `feed` with `snapshots`."""
        with self._lock:
            for key in self._feeds.get(feed, set()):
                self._markets.pop(key, None)
            self._feeds[feed] = {s.key for s in snapshots}
            for snap in snapshots:
                self._markets[snap.key] = snap

    def markets_for_feed(self, feed: str) -> list[MarketSnapshot]:
        with self._lock:
            return [
                self._markets[key]
                for key in self._feeds.get(feed, set())
                if key in self._markets
            ]

    def markets(
        self, source: str | None = None, sport: str | None = None
    ) -> list[MarketSnapshot]:
        with self._lock:
            values = list(self._markets.values())
        if source:
            values = [m for m in values if m.source == source]
        if sport:
            values = [m for m in values if m.sport.lower() == sport.lower()]
        return sorted(values, key=lambda m: (m.event_name, m.source))

    def market(self, key: str) -> MarketSnapshot | None:
        with self._lock:
            return self._markets.get(key)

    # --- opportunities ---

    def record_opportunities(self, found: list[Opportunity]) -> None:
        """Merge newly detected opportunities into history.

        A re-detected opportunity keeps its first_seen and refreshes
        last_seen/odds. Anything not seen within the TTL is marked inactive.
        """
        now = utcnow()
        with self._lock:
            for opp in found:
                existing = self._opportunities.get(opp.id)
                if existing is not None:
                    opp.first_seen = existing.first_seen
                opp.last_seen = now
                opp.active = True
                self._opportunities[opp.id] = opp
            for opp in self._opportunities.values():
                if opp.active and now - opp.last_seen > self._ttl:
                    opp.active = False

    def opportunities(
        self,
        active_only: bool = True,
        min_profit: float | None = None,
        sport: str | None = None,
        kind: str | None = None,
    ) -> list[Opportunity]:
        with self._lock:
            values = [o.model_copy(deep=True) for o in self._opportunities.values()]
        if active_only:
            values = [o for o in values if o.active]
        if min_profit is not None:
            values = [o for o in values if o.profit_margin >= min_profit]
        if sport:
            values = [o for o in values if o.sport.lower() == sport.lower()]
        if kind:
            values = [o for o in values if o.kind.value == kind]
        return sorted(values, key=lambda o: o.profit_margin, reverse=True)

    def opportunity(self, opp_id: str) -> Opportunity | None:
        with self._lock:
            opp = self._opportunities.get(opp_id)
            return opp.model_copy(deep=True) if opp else None

    def counts(self) -> tuple[int, int]:
        with self._lock:
            active = sum(1 for o in self._opportunities.values() if o.active)
            return len(self._markets), active

"""The Odds API source (https://the-odds-api.com/).

One aggregator that returns head-to-head odds from dozens of fixed-odds
bookmakers per event, so a single API key covers "other betting sites".
Each bookmaker's price is kept separately so the arbitrage engine can pick
the best price per outcome across bookmakers.
"""
from __future__ import annotations

import logging
from datetime import datetime

import httpx

from ..config import Settings
from ..models import MarketKind, MarketSnapshot, OutcomeOdds
from .base import OddsSource

logger = logging.getLogger(__name__)

BASE_URL = "https://api.the-odds-api.com/v4"


class OddsApiSource(OddsSource):
    name = "oddsapi"

    def __init__(self, settings: Settings) -> None:
        if not settings.odds_api_key:
            raise ValueError("ARB_ODDS_API_KEY is required for the oddsapi source")
        self._settings = settings
        self._client = httpx.AsyncClient(timeout=20.0, base_url=BASE_URL)

    async def stop(self) -> None:
        await self._client.aclose()

    async def fetch_markets(self) -> list[MarketSnapshot]:
        sports = [
            s.strip() for s in self._settings.odds_api_sports.split(",") if s.strip()
        ]
        snapshots: list[MarketSnapshot] = []
        for sport in sports:
            resp = await self._client.get(
                f"/sports/{sport}/odds",
                params={
                    "apiKey": self._settings.odds_api_key,
                    "regions": self._settings.odds_api_regions,
                    "markets": "h2h",
                    "oddsFormat": "decimal",
                },
            )
            resp.raise_for_status()
            remaining = resp.headers.get("x-requests-remaining")
            if remaining is not None:
                logger.debug("The Odds API requests remaining: %s", remaining)
            for event in resp.json():
                snapshot = self._to_snapshot(sport, event)
                if snapshot is not None:
                    snapshots.append(snapshot)
        return snapshots

    def _to_snapshot(self, sport: str, event: dict) -> MarketSnapshot | None:
        outcomes: list[OutcomeOdds] = []
        n_outcomes = 0
        for bookmaker in event.get("bookmakers", []):
            for market in bookmaker.get("markets", []):
                if market.get("key") != "h2h":
                    continue
                n_outcomes = max(n_outcomes, len(market.get("outcomes", [])))
                for outcome in market.get("outcomes", []):
                    outcomes.append(
                        OutcomeOdds(
                            outcome=outcome["name"],
                            bookmaker=bookmaker.get("key", "unknown"),
                            back_odds=float(outcome["price"]),
                        )
                    )
        if not outcomes:
            return None

        start_time = None
        if event.get("commence_time"):
            start_time = datetime.fromisoformat(
                event["commence_time"].replace("Z", "+00:00")
            )
        home = event.get("home_team", "")
        away = event.get("away_team", "")
        event_name = f"{home} v {away}" if home and away else event.get("id", "")
        return MarketSnapshot(
            source=self.name,
            market_id=event["id"],
            event_name=event_name,
            sport=event.get("sport_title", sport),
            kind=MarketKind.MATCH_ODDS if n_outcomes == 3 else MarketKind.HEAD_TO_HEAD,
            start_time=start_time,
            outcomes=outcomes,
        )

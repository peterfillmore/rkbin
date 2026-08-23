"""Mock source for demos and tests.

Emits a small set of fixtures priced by a fake exchange and a couple of fake
bookmakers. Odds drift randomly on every poll, so arbitrage opportunities
appear and disappear over time — useful for exercising the API and poller
without any credentials.
"""
from __future__ import annotations

import random
from datetime import timedelta

from ..models import MarketKind, MarketSnapshot, OutcomeOdds, utcnow
from .base import OddsSource

_FIXTURES = [
    {
        "event": "Arsenal v Chelsea",
        "sport": "Soccer",
        "kind": MarketKind.MATCH_ODDS,
        "outcomes": ["Arsenal", "The Draw", "Chelsea"],
        "fair": [2.10, 3.60, 3.80],
    },
    {
        "event": "Djokovic v Alcaraz",
        "sport": "Tennis",
        "kind": MarketKind.HEAD_TO_HEAD,
        "outcomes": ["Djokovic", "Alcaraz"],
        "fair": [2.30, 1.72],
    },
    {
        "event": "Melbourne v Sydney",
        "sport": "Australian Rules",
        "kind": MarketKind.HEAD_TO_HEAD,
        "outcomes": ["Melbourne", "Sydney"],
        "fair": [1.85, 2.05],
    },
]

_BOOKMAKERS = ["mockbet", "fakeodds"]


class MockSource(OddsSource):
    name = "mock"

    def __init__(self, seed: int | None = None) -> None:
        self._rng = random.Random(seed)

    def _jitter(self, price: float, spread: float) -> float:
        return round(max(1.01, price * self._rng.uniform(1 - spread, 1 + spread)), 2)

    async def fetch_markets(self) -> list[MarketSnapshot]:
        snapshots: list[MarketSnapshot] = []
        start = utcnow() + timedelta(hours=6)
        for i, fx in enumerate(_FIXTURES):
            # Fake exchange market: back/lay prices around fair odds.
            exchange_outcomes = []
            for name, fair in zip(fx["outcomes"], fx["fair"]):
                back = self._jitter(fair, 0.06)
                exchange_outcomes.append(
                    OutcomeOdds(
                        outcome=name,
                        bookmaker="mock_exchange",
                        back_odds=back,
                        lay_odds=round(back * self._rng.uniform(1.005, 1.03), 2),
                        is_exchange=True,
                    )
                )
            snapshots.append(
                MarketSnapshot(
                    source="mock_exchange",
                    market_id=f"ex-{i}",
                    event_name=fx["event"],
                    sport=fx["sport"],
                    kind=fx["kind"],
                    start_time=start,
                    outcomes=exchange_outcomes,
                )
            )

            # Fake fixed-odds bookmakers around the same fair prices.
            book_outcomes = [
                OutcomeOdds(
                    outcome=name,
                    bookmaker=bookie,
                    back_odds=self._jitter(fair, 0.08),
                )
                for bookie in _BOOKMAKERS
                for name, fair in zip(fx["outcomes"], fx["fair"])
            ]
            snapshots.append(
                MarketSnapshot(
                    source="mock_books",
                    market_id=f"bk-{i}",
                    event_name=fx["event"],
                    sport=fx["sport"],
                    kind=fx["kind"],
                    start_time=start,
                    outcomes=book_outcomes,
                )
            )
        return snapshots

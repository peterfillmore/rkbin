"""Domain models shared across sources, the arbitrage engine and the API."""
from __future__ import annotations

from datetime import datetime, timezone
from enum import Enum

from pydantic import BaseModel, Field


def utcnow() -> datetime:
    return datetime.now(timezone.utc)


class MarketKind(str, Enum):
    """Normalised market type across all sources."""

    MATCH_ODDS = "match_odds"  # 1X2 / head-to-head including draw
    HEAD_TO_HEAD = "h2h"       # two-outcome winner market
    OTHER = "other"


class OutcomeOdds(BaseModel):
    """Best available prices for one outcome at one source/bookmaker."""

    outcome: str
    bookmaker: str
    back_odds: float | None = None
    lay_odds: float | None = None
    # True when the price comes from an exchange and commission applies.
    is_exchange: bool = False


class MarketSnapshot(BaseModel):
    """One market at one source at one point in time."""

    source: str
    market_id: str
    event_name: str
    sport: str = ""
    competition: str = ""
    kind: MarketKind = MarketKind.OTHER
    start_time: datetime | None = None
    outcomes: list[OutcomeOdds] = Field(default_factory=list)
    fetched_at: datetime = Field(default_factory=utcnow)

    @property
    def key(self) -> str:
        return f"{self.source}:{self.market_id}"


class OpportunityKind(str, Enum):
    CROSS_BOOK = "cross_book"  # back every outcome at different bookmakers
    BACK_LAY = "back_lay"      # back at a bookmaker, lay on the exchange


class OpportunityLeg(BaseModel):
    outcome: str
    bookmaker: str
    source: str
    side: str  # "back" or "lay"
    odds: float
    # Fraction of total outlay to place on this leg (sums to 1 across legs).
    stake_fraction: float


class Opportunity(BaseModel):
    id: str
    kind: OpportunityKind
    event_name: str
    sport: str = ""
    market_kind: MarketKind
    start_time: datetime | None = None
    legs: list[OpportunityLeg]
    # Guaranteed profit per unit of total stake (e.g. 0.021 => 2.1%).
    profit_margin: float
    first_seen: datetime = Field(default_factory=utcnow)
    last_seen: datetime = Field(default_factory=utcnow)
    active: bool = True


class PollerStatus(BaseModel):
    running: bool
    poll_interval_seconds: float
    last_poll_started_at: datetime | None = None
    last_poll_finished_at: datetime | None = None
    last_poll_duration_seconds: float | None = None
    polls_completed: int = 0
    last_error: str | None = None
    markets_tracked: int = 0
    active_opportunities: int = 0
    sources: list[str] = Field(default_factory=list)

"""Abstract interface every odds source implements."""
from __future__ import annotations

import abc

from ..models import MarketSnapshot


class OddsSource(abc.ABC):
    """A provider of market odds (an exchange, a bookmaker, or an aggregator)."""

    #: Short identifier used in MarketSnapshot.source and API responses.
    name: str = "base"

    async def start(self) -> None:
        """Open connections / authenticate. Called once before polling begins."""

    async def stop(self) -> None:
        """Release connections. Called on shutdown."""

    @abc.abstractmethod
    async def fetch_markets(self) -> list[MarketSnapshot]:
        """Return the current snapshot of all markets this source tracks.

        Implementations should raise on hard failures; the poller catches
        per-source errors so one failing source does not stop the others.
        """

from .base import OddsSource
from .betfair import BetfairSource
from .mock import MockSource
from .oddsapi import OddsApiSource

from ..config import Settings


def build_sources(settings: Settings) -> list[OddsSource]:
    """Instantiate the sources named in settings.enabled_sources."""
    registry = {
        "mock": lambda: MockSource(),
        "betfair": lambda: BetfairSource(settings),
        "oddsapi": lambda: OddsApiSource(settings),
    }
    sources: list[OddsSource] = []
    for name in settings.enabled_source_names:
        factory = registry.get(name)
        if factory is None:
            raise ValueError(
                f"Unknown source '{name}'. Valid sources: {sorted(registry)}"
            )
        sources.append(factory())
    return sources


__all__ = [
    "OddsSource",
    "BetfairSource",
    "OddsApiSource",
    "MockSource",
    "build_sources",
]

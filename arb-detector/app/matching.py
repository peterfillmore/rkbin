"""Group market snapshots from different sources that refer to the same
real-world event, and align outcome names across sources.

Bookmakers spell things differently ("Man Utd" vs "Manchester United",
"Draw" vs "The Draw"), so matching is fuzzy: normalised-name similarity plus
a start-time window.
"""
from __future__ import annotations

import re
from dataclasses import dataclass, field
from difflib import SequenceMatcher

from .models import MarketSnapshot

_SEPARATORS = re.compile(r"\s+(?:vs\.?|v\.?|@|-)\s+", re.IGNORECASE)
_NON_ALNUM = re.compile(r"[^a-z0-9 ]+")
_NOISE_WORDS = {"fc", "afc", "cf", "sc", "the", "utd", "united"}
_DRAW_NAMES = {"draw", "the draw", "tie"}


def normalize_name(name: str) -> str:
    text = _NON_ALNUM.sub(" ", name.lower())
    tokens = [t for t in text.split() if t not in _NOISE_WORDS]
    return " ".join(tokens)


def split_event_teams(event_name: str) -> list[str]:
    """Split "Arsenal v Chelsea" into normalised participant names."""
    parts = _SEPARATORS.split(event_name)
    return [normalize_name(p) for p in parts if p.strip()]


def name_similarity(a: str, b: str) -> float:
    a, b = normalize_name(a), normalize_name(b)
    if not a or not b:
        return 0.0
    if a == b or a in b or b in a:
        return 1.0
    return SequenceMatcher(None, a, b).ratio()


def is_draw(outcome: str) -> bool:
    return normalize_name(outcome) in _DRAW_NAMES or outcome.strip().lower() in _DRAW_NAMES


def events_match(
    a: MarketSnapshot,
    b: MarketSnapshot,
    threshold: float,
    max_start_diff_minutes: float,
) -> bool:
    if a.kind != b.kind:
        return False
    if a.start_time and b.start_time:
        diff = abs((a.start_time - b.start_time).total_seconds()) / 60.0
        if diff > max_start_diff_minutes:
            return False

    teams_a = split_event_teams(a.event_name)
    teams_b = split_event_teams(b.event_name)
    if len(teams_a) == 2 and len(teams_b) == 2:
        # Compare participants pairwise; allow either order (home/away flips).
        straight = min(
            name_similarity(teams_a[0], teams_b[0]),
            name_similarity(teams_a[1], teams_b[1]),
        )
        flipped = min(
            name_similarity(teams_a[0], teams_b[1]),
            name_similarity(teams_a[1], teams_b[0]),
        )
        return max(straight, flipped) >= threshold
    return name_similarity(a.event_name, b.event_name) >= threshold


def match_outcome(name: str, candidates: list[str], threshold: float) -> str | None:
    """Return the candidate outcome that `name` refers to, or None."""
    if is_draw(name):
        for c in candidates:
            if is_draw(c):
                return c
        return None
    best, best_score = None, 0.0
    for c in candidates:
        if is_draw(c):
            continue
        score = name_similarity(name, c)
        if score > best_score:
            best, best_score = c, score
    return best if best_score >= threshold else None


@dataclass
class EventGroup:
    """All market snapshots (across sources) for one real-world event."""

    representative: MarketSnapshot
    markets: list[MarketSnapshot] = field(default_factory=list)

    @property
    def canonical_outcomes(self) -> list[str]:
        return [o.outcome for o in self.representative.outcomes]


def group_markets(
    snapshots: list[MarketSnapshot],
    threshold: float = 0.75,
    max_start_diff_minutes: float = 30.0,
) -> list[EventGroup]:
    groups: list[EventGroup] = []
    for snap in snapshots:
        placed = False
        for group in groups:
            if events_match(
                snap, group.representative, threshold, max_start_diff_minutes
            ):
                group.markets.append(snap)
                placed = True
                break
        if not placed:
            groups.append(EventGroup(representative=snap, markets=[snap]))
    return groups

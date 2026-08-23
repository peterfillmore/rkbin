from datetime import timedelta

from app.matching import (
    events_match,
    group_markets,
    match_outcome,
    normalize_name,
    split_event_teams,
)
from app.models import MarketKind, MarketSnapshot, OutcomeOdds, utcnow


def market(source, event, kind=MarketKind.HEAD_TO_HEAD, start=None, outcomes=()):
    return MarketSnapshot(
        source=source,
        market_id=f"{source}-{event}",
        event_name=event,
        kind=kind,
        start_time=start,
        outcomes=list(outcomes),
    )


def test_normalize_strips_noise():
    assert normalize_name("Manchester Utd FC") == "manchester"
    assert normalize_name("Real Madrid") == "real madrid"


def test_split_event_teams_handles_separators():
    assert split_event_teams("Arsenal v Chelsea") == ["arsenal", "chelsea"]
    assert split_event_teams("Arsenal vs. Chelsea") == ["arsenal", "chelsea"]
    assert split_event_teams("Lakers @ Celtics") == ["lakers", "celtics"]


def test_events_match_fuzzy_names_and_flipped_order():
    now = utcnow()
    a = market("s1", "Man City v Liverpool", start=now)
    b = market("s2", "Liverpool vs Manchester City", start=now)
    assert events_match(a, b, threshold=0.6, max_start_diff_minutes=30)


def test_events_do_not_match_across_kinds_or_far_start_times():
    now = utcnow()
    a = market("s1", "Arsenal v Chelsea", kind=MarketKind.MATCH_ODDS, start=now)
    b = market("s2", "Arsenal v Chelsea", kind=MarketKind.HEAD_TO_HEAD, start=now)
    assert not events_match(a, b, threshold=0.75, max_start_diff_minutes=30)

    c = market("s2", "Arsenal v Chelsea", kind=MarketKind.MATCH_ODDS,
               start=now + timedelta(hours=5))
    assert not events_match(a, c, threshold=0.75, max_start_diff_minutes=30)


def test_match_outcome_draw_aliases_and_fuzzy_teams():
    candidates = ["Arsenal", "The Draw", "Chelsea"]
    assert match_outcome("Draw", candidates, 0.75) == "The Draw"
    assert match_outcome("Arsenal FC", candidates, 0.75) == "Arsenal"
    assert match_outcome("Tottenham", candidates, 0.75) is None


def test_group_markets_groups_same_event():
    now = utcnow()
    snaps = [
        market("betfair", "Djokovic v Alcaraz", start=now,
               outcomes=[OutcomeOdds(outcome="Djokovic", bookmaker="betfair",
                                     back_odds=2.0, is_exchange=True)]),
        market("oddsapi", "N. Djokovic vs C. Alcaraz", start=now,
               outcomes=[OutcomeOdds(outcome="Djokovic", bookmaker="bk",
                                     back_odds=2.1)]),
        market("oddsapi", "Sinner vs Zverev", start=now),
    ]
    groups = group_markets(snaps, threshold=0.6)
    assert len(groups) == 2
    assert len(groups[0].markets) == 2

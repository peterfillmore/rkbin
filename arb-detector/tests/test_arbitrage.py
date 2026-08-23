import pytest

from app.arbitrage import (
    detect_back_lay,
    detect_cross_book,
    effective_back_odds,
    find_opportunities,
)
from app.matching import EventGroup
from app.models import MarketKind, MarketSnapshot, OutcomeOdds


def make_market(source, market_id, event, outcomes, kind=MarketKind.HEAD_TO_HEAD):
    return MarketSnapshot(
        source=source,
        market_id=market_id,
        event_name=event,
        sport="Tennis",
        kind=kind,
        outcomes=outcomes,
    )


def group_of(*markets):
    return EventGroup(representative=markets[0], markets=list(markets))


def test_effective_back_odds_applies_commission_to_exchange_only():
    exchange = OutcomeOdds(outcome="A", bookmaker="bf", back_odds=3.0, is_exchange=True)
    bookie = OutcomeOdds(outcome="A", bookmaker="bk", back_odds=3.0)
    assert effective_back_odds(exchange, 0.05) == pytest.approx(2.9)
    assert effective_back_odds(bookie, 0.05) == 3.0


def test_cross_book_arb_detected():
    # 2.10 and 2.10: 1/2.1 + 1/2.1 = 0.952 -> ~5% arb
    m1 = make_market(
        "s1", "m1", "Djokovic v Alcaraz",
        [OutcomeOdds(outcome="Djokovic", bookmaker="bk1", back_odds=2.10),
         OutcomeOdds(outcome="Alcaraz", bookmaker="bk1", back_odds=1.60)],
    )
    m2 = make_market(
        "s2", "m2", "Djokovic v Alcaraz",
        [OutcomeOdds(outcome="Djokovic", bookmaker="bk2", back_odds=1.70),
         OutcomeOdds(outcome="Alcaraz", bookmaker="bk2", back_odds=2.10)],
    )
    opp = detect_cross_book(group_of(m1, m2), commission=0.05, min_profit_margin=0.005)
    assert opp is not None
    assert opp.profit_margin == pytest.approx(1 / (2 / 2.10) - 1, abs=1e-6)
    assert {leg.bookmaker for leg in opp.legs} == {"bk1", "bk2"}
    # Equal odds -> equal stakes, summing to 1.
    assert sum(leg.stake_fraction for leg in opp.legs) == pytest.approx(1.0, abs=1e-4)
    # Verify the arb is genuine: every result returns more than the outlay.
    for leg in opp.legs:
        assert leg.stake_fraction * leg.odds == pytest.approx(
            1 + opp.profit_margin, abs=1e-4
        )


def test_no_arb_when_overround():
    m1 = make_market(
        "s1", "m1", "Djokovic v Alcaraz",
        [OutcomeOdds(outcome="Djokovic", bookmaker="bk1", back_odds=1.90),
         OutcomeOdds(outcome="Alcaraz", bookmaker="bk1", back_odds=1.90)],
    )
    m2 = make_market(
        "s2", "m2", "Djokovic v Alcaraz",
        [OutcomeOdds(outcome="Djokovic", bookmaker="bk2", back_odds=1.85),
         OutcomeOdds(outcome="Alcaraz", bookmaker="bk2", back_odds=1.85)],
    )
    assert detect_cross_book(
        group_of(m1, m2), commission=0.05, min_profit_margin=0.005
    ) is None


def test_single_bookmaker_not_reported():
    m1 = make_market(
        "s1", "m1", "Djokovic v Alcaraz",
        [OutcomeOdds(outcome="Djokovic", bookmaker="bk1", back_odds=2.20),
         OutcomeOdds(outcome="Alcaraz", bookmaker="bk1", back_odds=2.20)],
    )
    assert detect_cross_book(
        group_of(m1), commission=0.05, min_profit_margin=0.005
    ) is None


def test_incomplete_outcome_coverage_not_reported():
    m1 = make_market(
        "s1", "m1", "Arsenal v Chelsea",
        [OutcomeOdds(outcome="Arsenal", bookmaker="bk1", back_odds=4.0),
         OutcomeOdds(outcome="Chelsea", bookmaker="bk1", back_odds=4.0)],
        kind=MarketKind.MATCH_ODDS,  # 3-way market but only 2 outcomes priced
    )
    assert detect_cross_book(
        group_of(m1), commission=0.05, min_profit_margin=0.005
    ) is None


def test_back_lay_arb_detected_and_profit_is_balanced():
    commission = 0.05
    bookie = make_market(
        "books", "m1", "Djokovic v Alcaraz",
        [OutcomeOdds(outcome="Djokovic", bookmaker="bk1", back_odds=2.40)],
    )
    exchange = make_market(
        "betfair", "m2", "Djokovic v Alcaraz",
        [OutcomeOdds(outcome="Djokovic", bookmaker="betfair",
                     back_odds=2.16, lay_odds=2.20, is_exchange=True)],
    )
    opps = detect_back_lay(
        group_of(bookie, exchange), commission=commission, min_profit_margin=0.001
    )
    assert len(opps) == 1
    opp = opps[0]
    back = next(l for l in opp.legs if l.side == "back")
    lay = next(l for l in opp.legs if l.side == "lay")
    assert back.bookmaker == "bk1" and lay.bookmaker == "betfair"

    # Recompute both result branches from the reported stakes and check they
    # match the reported margin (profit per unit of capital incl. liability).
    s, l = back.stake_fraction, lay.stake_fraction
    win = s * (back.odds - 1) - l * (lay.odds - 1)
    lose = -s + l * (1 - commission)
    assert win == pytest.approx(lose, abs=1e-4)
    capital = s + l * (lay.odds - 1)
    assert win / capital == pytest.approx(opp.profit_margin, abs=1e-3)


def test_back_lay_no_arb_when_lay_too_high():
    bookie = make_market(
        "books", "m1", "Djokovic v Alcaraz",
        [OutcomeOdds(outcome="Djokovic", bookmaker="bk1", back_odds=2.00)],
    )
    exchange = make_market(
        "betfair", "m2", "Djokovic v Alcaraz",
        [OutcomeOdds(outcome="Djokovic", bookmaker="betfair",
                     back_odds=2.05, lay_odds=2.10, is_exchange=True)],
    )
    assert detect_back_lay(
        group_of(bookie, exchange), commission=0.05, min_profit_margin=0.001
    ) == []


def test_find_opportunities_sorted_by_margin():
    m1 = make_market(
        "s1", "m1", "Djokovic v Alcaraz",
        [OutcomeOdds(outcome="Djokovic", bookmaker="bk1", back_odds=2.30),
         OutcomeOdds(outcome="Alcaraz", bookmaker="bk1", back_odds=1.60)],
    )
    m2 = make_market(
        "s2", "m2", "Djokovic v Alcaraz",
        [OutcomeOdds(outcome="Djokovic", bookmaker="bk2", back_odds=1.70),
         OutcomeOdds(outcome="Alcaraz", bookmaker="bk2", back_odds=2.30)],
    )
    from app.matching import group_markets

    groups = group_markets([m1, m2])
    opps = find_opportunities(groups, commission=0.05, min_profit_margin=0.001)
    assert opps
    margins = [o.profit_margin for o in opps]
    assert margins == sorted(margins, reverse=True)

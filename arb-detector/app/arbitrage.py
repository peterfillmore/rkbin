"""Arbitrage detection.

Two kinds of opportunity are detected over a group of markets that refer to
the same real-world event:

* cross_book — back every outcome of the market, each at the bookmaker
  offering the best price for that outcome. An arb exists when the sum of
  inverse (commission-adjusted) odds is below 1.

* back_lay — back one outcome at a fixed-odds bookmaker and lay the same
  outcome on the exchange. An arb exists when
  back_odds * (1 - commission) > lay_odds - commission.

Exchange prices are adjusted for commission, which is charged on net
winnings: effective_back = 1 + (odds - 1) * (1 - commission).
Profit margins are expressed per unit of total capital outlaid (for
back/lay, the lay leg's capital is its liability, not its stake).
"""
from __future__ import annotations

import hashlib
from dataclasses import dataclass

from .matching import EventGroup, match_outcome
from .models import (
    MarketKind,
    Opportunity,
    OpportunityKind,
    OpportunityLeg,
    OutcomeOdds,
)

_EXPECTED_OUTCOMES = {MarketKind.MATCH_ODDS: 3, MarketKind.HEAD_TO_HEAD: 2}


@dataclass(frozen=True)
class Quote:
    """One outcome price tied back to its source market."""

    outcome: str  # canonical outcome name for the event group
    source: str
    odds: OutcomeOdds


def _opportunity_id(kind: str, event: str, parts: list[str]) -> str:
    raw = "|".join([kind, event.lower(), *sorted(parts)])
    return hashlib.sha1(raw.encode()).hexdigest()[:12]


def effective_back_odds(odds: OutcomeOdds, commission: float) -> float | None:
    if odds.back_odds is None:
        return None
    if odds.is_exchange:
        return 1 + (odds.back_odds - 1) * (1 - commission)
    return odds.back_odds


def collect_quotes(group: EventGroup, outcome_threshold: float = 0.75) -> list[Quote]:
    """Flatten a group's markets into quotes keyed by canonical outcome name."""
    canonical = group.canonical_outcomes
    quotes: list[Quote] = []
    for market in group.markets:
        for odds in market.outcomes:
            matched = match_outcome(odds.outcome, canonical, outcome_threshold)
            if matched is not None:
                quotes.append(Quote(outcome=matched, source=market.source, odds=odds))
    return quotes


def detect_cross_book(
    group: EventGroup,
    commission: float,
    min_profit_margin: float,
    outcome_threshold: float = 0.75,
) -> Opportunity | None:
    expected = _EXPECTED_OUTCOMES.get(group.representative.kind)
    if expected is None:
        return None

    quotes = collect_quotes(group, outcome_threshold)
    best: dict[str, tuple[Quote, float]] = {}
    for quote in quotes:
        eff = effective_back_odds(quote.odds, commission)
        if eff is None:
            continue
        current = best.get(quote.outcome)
        if current is None or eff > current[1]:
            best[quote.outcome] = (quote, eff)

    if len(best) != expected:
        return None
    bookmakers = {q.odds.bookmaker for q, _ in best.values()}
    if len(bookmakers) < 2:
        # A single book's own overround is not an actionable arb.
        return None

    inverse_sum = sum(1 / eff for _, eff in best.values())
    if inverse_sum >= 1.0:
        return None
    margin = 1.0 / inverse_sum - 1.0
    if margin < min_profit_margin:
        return None

    rep = group.representative
    legs = [
        OpportunityLeg(
            outcome=outcome,
            bookmaker=quote.odds.bookmaker,
            source=quote.source,
            side="back",
            odds=quote.odds.back_odds,
            stake_fraction=round((1 / eff) / inverse_sum, 6),
        )
        for outcome, (quote, eff) in sorted(best.items())
    ]
    return Opportunity(
        id=_opportunity_id(
            "cross_book",
            rep.event_name,
            [f"{leg.outcome}@{leg.bookmaker}" for leg in legs],
        ),
        kind=OpportunityKind.CROSS_BOOK,
        event_name=rep.event_name,
        sport=rep.sport,
        market_kind=rep.kind,
        start_time=rep.start_time,
        legs=legs,
        profit_margin=round(margin, 6),
    )


def detect_back_lay(
    group: EventGroup,
    commission: float,
    min_profit_margin: float,
    outcome_threshold: float = 0.75,
) -> list[Opportunity]:
    quotes = collect_quotes(group, outcome_threshold)

    best_back: dict[str, Quote] = {}   # best fixed-odds (non-exchange) back price
    best_lay: dict[str, Quote] = {}    # lowest exchange lay price
    for quote in quotes:
        if quote.odds.is_exchange:
            if quote.odds.lay_odds is not None:
                current = best_lay.get(quote.outcome)
                if current is None or quote.odds.lay_odds < current.odds.lay_odds:
                    best_lay[quote.outcome] = quote
        elif quote.odds.back_odds is not None:
            current = best_back.get(quote.outcome)
            if current is None or quote.odds.back_odds > current.odds.back_odds:
                best_back[quote.outcome] = quote

    rep = group.representative
    opportunities: list[Opportunity] = []
    for outcome in best_back.keys() & best_lay.keys():
        back = best_back[outcome]
        lay = best_lay[outcome]
        b = back.odds.back_odds
        o = lay.odds.lay_odds
        if o <= commission:  # degenerate, cannot happen with real prices
            continue
        # Lay stake that equalises profit across both results, per unit backed.
        lay_stake = b / (o - commission)
        profit_per_back_unit = b * (1 - commission) / (o - commission) - 1
        if profit_per_back_unit <= 0:
            continue
        capital = 1.0 + lay_stake * (o - 1)  # back stake + lay liability
        margin = profit_per_back_unit / capital
        if margin < min_profit_margin:
            continue

        total_stakes = 1.0 + lay_stake
        legs = [
            OpportunityLeg(
                outcome=outcome,
                bookmaker=back.odds.bookmaker,
                source=back.source,
                side="back",
                odds=b,
                stake_fraction=round(1.0 / total_stakes, 6),
            ),
            OpportunityLeg(
                outcome=outcome,
                bookmaker=lay.odds.bookmaker,
                source=lay.source,
                side="lay",
                odds=o,
                stake_fraction=round(lay_stake / total_stakes, 6),
            ),
        ]
        opportunities.append(
            Opportunity(
                id=_opportunity_id(
                    "back_lay",
                    rep.event_name,
                    [f"{outcome}@{back.odds.bookmaker}>{lay.odds.bookmaker}"],
                ),
                kind=OpportunityKind.BACK_LAY,
                event_name=rep.event_name,
                sport=rep.sport,
                market_kind=rep.kind,
                start_time=rep.start_time,
                legs=legs,
                profit_margin=round(margin, 6),
            )
        )
    return opportunities


def find_opportunities(
    groups: list[EventGroup],
    commission: float,
    min_profit_margin: float,
    outcome_threshold: float = 0.75,
) -> list[Opportunity]:
    found: list[Opportunity] = []
    for group in groups:
        cross = detect_cross_book(
            group, commission, min_profit_margin, outcome_threshold
        )
        if cross is not None:
            found.append(cross)
        found.extend(
            detect_back_lay(group, commission, min_profit_margin, outcome_threshold)
        )
    found.sort(key=lambda o: o.profit_margin, reverse=True)
    return found

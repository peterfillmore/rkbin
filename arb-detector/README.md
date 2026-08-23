# Betting Arbitrage Detector

A Python REST API that polls the **Betfair Exchange API** and fixed-odds
bookmakers (via [The Odds API](https://the-odds-api.com/), which aggregates
dozens of betting sites) in the background, fuzzy-matches markets that refer
to the same real-world event, and exposes detected **arbitrage
opportunities** over REST.

Two kinds of arbitrage are detected:

| Kind | Description | Condition |
|---|---|---|
| `cross_book` | Back **every** outcome of a market, each at the bookmaker with the best price | `sum(1 / odds_i) < 1` (exchange odds commission-adjusted) |
| `back_lay` | Back an outcome at a bookmaker, lay the same outcome on Betfair | `back × (1 − commission) > lay − commission` |

Every opportunity includes per-leg **stake fractions** that equalise profit
across all results, and a **profit margin** per unit of total capital
(including lay liability for back/lay arbs).

## Quick start (no credentials needed)

The default configuration uses a built-in mock source with drifting odds, so
you can run the whole thing immediately:

```bash
cd arb-detector
pip install -r requirements.txt
uvicorn app.main:app --reload
```

Then:

```bash
curl localhost:8000/status                  # poller state
curl localhost:8000/opportunities           # current arbs, best first
curl localhost:8000/markets                 # all tracked markets
```

Interactive OpenAPI docs: http://localhost:8000/docs

## Real data sources

Copy `.env.example` to `.env` and configure:

```ini
ARB_ENABLED_SOURCES=betfair,oddsapi

# Betfair Exchange (https://developer.betfair.com/)
ARB_BETFAIR_APP_KEY=...
ARB_BETFAIR_USERNAME=...
ARB_BETFAIR_PASSWORD=...
# optional client certificate for non-interactive bot login:
ARB_BETFAIR_CERT_FILE=client-2048.crt
ARB_BETFAIR_KEY_FILE=client-2048.key
ARB_BETFAIR_EVENT_TYPE_IDS=1,2        # 1=Soccer, 2=Tennis

# The Odds API (https://the-odds-api.com/) — covers dozens of bookmakers
ARB_ODDS_API_KEY=...
ARB_ODDS_API_SPORTS=soccer_epl,tennis_atp_french_open
ARB_ODDS_API_REGIONS=au,uk,eu
```

Additional sources are pluggable: subclass `app.sources.base.OddsSource`,
implement `fetch_markets()` returning `MarketSnapshot`s, and register it in
`app/sources/__init__.py`.

## REST API

| Method | Path | Description |
|---|---|---|
| GET | `/health` | Liveness check |
| GET | `/status` | Poller state: last poll, duration, counts, errors |
| GET | `/markets?source=&sport=` | All tracked market snapshots |
| GET | `/markets/{source}/{market_id}` | One market |
| GET | `/opportunities?active=&min_profit=&sport=&kind=` | Detected arbs, best margin first |
| GET | `/opportunities/{id}` | One opportunity (kept as history after expiry) |
| POST | `/poller/start` | Start background polling |
| POST | `/poller/stop` | Stop background polling |
| POST | `/poller/poll` | Force an immediate poll |
| PATCH | `/poller/config` | Change the poll interval at runtime |
| WS | `/ws` | Live stream: snapshot on connect, then per-poll updates |

### WebSocket stream

Connect to `ws://host:8000/ws` to get pushed updates instead of polling the
REST endpoints. On connect the server sends the current state:

```json
{"type": "snapshot", "status": { ... }, "opportunities": [ ... ]}
```

Then, after every background poll, one message:

```json
{
  "type": "poll",
  "status": { "polls_completed": 42, "markets_tracked": 6, ... },
  "new":     [ ...full opportunities first seen this poll... ],
  "changed": [ ...opportunities whose profit margin moved... ],
  "expired": [ "ids no longer active" ]
}
```

Minimal client:

```python
import asyncio, json, websockets

async def watch():
    async with websockets.connect("ws://localhost:8000/ws") as ws:
        async for raw in ws:
            msg = json.loads(raw)
            for opp in msg.get("new", []) + msg.get("opportunities", []):
                print(f"{opp['profit_margin']:.2%}  {opp['event_name']}")

asyncio.run(watch())
```

Slow consumers never stall the poller: each client has a bounded queue and
the oldest unsent messages are dropped first.

Example opportunity:

```json
{
  "id": "9f2c1a7b03de",
  "kind": "cross_book",
  "event_name": "Djokovic v Alcaraz",
  "market_kind": "h2h",
  "profit_margin": 0.022,
  "legs": [
    {"outcome": "Djokovic", "bookmaker": "mockbet",  "side": "back", "odds": 2.40, "stake_fraction": 0.4258},
    {"outcome": "Alcaraz",  "bookmaker": "fakeodds", "side": "back", "odds": 1.78, "stake_fraction": 0.5742}
  ]
}
```

Stake fractions sum to 1: with a $1000 bankroll, stake $425.80 and $574.20
respectively and the return is ~$1022 whichever player wins.

## How it works

1. **Poller** (`app/poller.py`) — an asyncio background task started on app
   startup. Every `ARB_POLL_INTERVAL_SECONDS` it fetches all enabled sources
   concurrently; a failing source keeps its previous snapshot and is reported
   in `/status.last_error` without stopping the others.
2. **Matching** (`app/matching.py`) — markets from different sources are
   grouped per event using normalised participant names (fuzzy matching,
   home/away flips, "Draw"/"The Draw" aliases) plus a start-time window.
3. **Detection** (`app/arbitrage.py`) — per event group, the best
   commission-adjusted price per outcome is selected across bookmakers and
   both arb conditions are evaluated. Detected opportunities are merged into
   the store: re-detections refresh `last_seen`, and anything not re-seen
   within `ARB_OPPORTUNITY_TTL_SECONDS` is flagged `active: false`.

## Configuration reference

All settings come from environment variables (prefix `ARB_`) or `.env` —
see `.env.example`. Key ones:

- `ARB_POLL_INTERVAL_SECONDS` (30) — background poll cadence
- `ARB_MIN_PROFIT_MARGIN` (0.005) — minimum edge to report
- `ARB_BETFAIR_COMMISSION` (0.05) — exchange commission on net winnings
- `ARB_EVENT_MATCH_THRESHOLD` (0.75) — fuzzy-match strictness
- `ARB_ENABLED_SOURCES` (mock) — comma-separated: `mock`, `betfair`, `oddsapi`

## Tests

```bash
python -m pytest
```

Covers the arbitrage maths (including verifying that reported stakes really
equalise profit on every result), event/outcome fuzzy matching, and the full
REST surface against the mock source.

## Caveats

- Prices move fast; an arb visible at poll time can be gone before bets are
  placed. Treat the output as a signal, not an execution engine.
- Betfair displayed prices carry limited volume; this service reads best
  available prices but does not check depth beyond the top of the book.
- Bookmakers restrict accounts that arb. Know the terms of the services you
  use and the rules that apply in your jurisdiction.

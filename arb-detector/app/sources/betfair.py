"""Betfair Exchange API source.

Uses the interactive login (username/password) or the certificate login when a
client certificate pair is configured, then reads MATCH_ODDS markets via the
Betting API (listMarketCatalogue + listMarketBook).

Docs: https://developer.betfair.com/en/get-started/
"""
from __future__ import annotations

import logging
from datetime import datetime

import httpx

from ..config import Settings
from ..models import MarketKind, MarketSnapshot, OutcomeOdds
from .base import OddsSource

logger = logging.getLogger(__name__)

INTERACTIVE_LOGIN_URL = "https://identitysso.betfair.com/api/login"
CERT_LOGIN_URL = "https://identitysso-cert.betfair.com/api/certlogin"
KEEP_ALIVE_URL = "https://identitysso.betfair.com/api/keepAlive"
BETTING_URL = "https://api.betfair.com/exchange/betting/rest/v1.0"

# listMarketBook accepts at most 40 market ids per request when asking for
# EX_BEST_OFFERS price data.
MARKET_BOOK_BATCH = 40


class BetfairAuthError(RuntimeError):
    pass


class BetfairSource(OddsSource):
    name = "betfair"

    def __init__(self, settings: Settings) -> None:
        if not settings.betfair_app_key:
            raise ValueError("ARB_BETFAIR_APP_KEY is required for the betfair source")
        self._settings = settings
        self._session_token: str | None = None
        cert = None
        if settings.betfair_cert_file and settings.betfair_key_file:
            cert = (settings.betfair_cert_file, settings.betfair_key_file)
        self._client = httpx.AsyncClient(timeout=20.0, cert=cert)

    async def start(self) -> None:
        await self._login()

    async def stop(self) -> None:
        await self._client.aclose()

    async def _login(self) -> None:
        s = self._settings
        use_cert = bool(s.betfair_cert_file and s.betfair_key_file)
        url = CERT_LOGIN_URL if use_cert else INTERACTIVE_LOGIN_URL
        resp = await self._client.post(
            url,
            data={"username": s.betfair_username, "password": s.betfair_password},
            headers={"X-Application": s.betfair_app_key, "Accept": "application/json"},
        )
        resp.raise_for_status()
        body = resp.json()
        # Cert login returns {"sessionToken": ..., "loginStatus": "SUCCESS"};
        # interactive login returns {"token": ..., "status": "SUCCESS"}.
        token = body.get("sessionToken") or body.get("token")
        status = body.get("loginStatus") or body.get("status")
        if status != "SUCCESS" or not token:
            raise BetfairAuthError(f"Betfair login failed: {status!r}")
        self._session_token = token
        logger.info("Betfair login successful")

    def _headers(self) -> dict[str, str]:
        return {
            "X-Application": self._settings.betfair_app_key,
            "X-Authentication": self._session_token or "",
            "Content-Type": "application/json",
            "Accept": "application/json",
        }

    async def _betting_post(self, method: str, payload: dict) -> list | dict:
        resp = await self._client.post(
            f"{BETTING_URL}/{method}/", json=payload, headers=self._headers()
        )
        if resp.status_code in (401, 403):
            # Session expired: re-login once and retry.
            await self._login()
            resp = await self._client.post(
                f"{BETTING_URL}/{method}/", json=payload, headers=self._headers()
            )
        resp.raise_for_status()
        return resp.json()

    async def fetch_markets(self) -> list[MarketSnapshot]:
        if self._session_token is None:
            await self._login()

        event_type_ids = [
            e.strip()
            for e in self._settings.betfair_event_type_ids.split(",")
            if e.strip()
        ]
        catalogue = await self._betting_post(
            "listMarketCatalogue",
            {
                "filter": {
                    "eventTypeIds": event_type_ids,
                    "marketTypeCodes": ["MATCH_ODDS"],
                    "inPlayOnly": False,
                },
                "marketProjection": [
                    "EVENT",
                    "COMPETITION",
                    "EVENT_TYPE",
                    "RUNNER_DESCRIPTION",
                    "MARKET_START_TIME",
                ],
                "sort": "FIRST_TO_START",
                "maxResults": self._settings.betfair_max_markets,
            },
        )
        if not catalogue:
            return []

        by_market: dict[str, dict] = {m["marketId"]: m for m in catalogue}
        market_ids = list(by_market)

        snapshots: list[MarketSnapshot] = []
        for i in range(0, len(market_ids), MARKET_BOOK_BATCH):
            batch = market_ids[i : i + MARKET_BOOK_BATCH]
            books = await self._betting_post(
                "listMarketBook",
                {
                    "marketIds": batch,
                    "priceProjection": {"priceData": ["EX_BEST_OFFERS"]},
                },
            )
            for book in books:
                snapshot = self._to_snapshot(by_market[book["marketId"]], book)
                if snapshot is not None:
                    snapshots.append(snapshot)
        return snapshots

    def _to_snapshot(self, cat: dict, book: dict) -> MarketSnapshot | None:
        if book.get("status") not in (None, "OPEN"):
            return None
        runner_names = {
            r["selectionId"]: r["runnerName"] for r in cat.get("runners", [])
        }
        outcomes: list[OutcomeOdds] = []
        for runner in book.get("runners", []):
            if runner.get("status") != "ACTIVE":
                continue
            name = runner_names.get(runner["selectionId"])
            if name is None:
                continue
            ex = runner.get("ex", {})
            backs = ex.get("availableToBack") or []
            lays = ex.get("availableToLay") or []
            outcomes.append(
                OutcomeOdds(
                    outcome=name,
                    bookmaker="betfair",
                    back_odds=backs[0]["price"] if backs else None,
                    lay_odds=lays[0]["price"] if lays else None,
                    is_exchange=True,
                )
            )
        if not outcomes:
            return None

        start_time = None
        if cat.get("marketStartTime"):
            start_time = datetime.fromisoformat(
                cat["marketStartTime"].replace("Z", "+00:00")
            )
        n_runners = len(cat.get("runners", []))
        return MarketSnapshot(
            source=self.name,
            market_id=cat["marketId"],
            event_name=cat.get("event", {}).get("name", cat.get("marketName", "")),
            sport=cat.get("eventType", {}).get("name", ""),
            competition=cat.get("competition", {}).get("name", ""),
            kind=MarketKind.MATCH_ODDS if n_runners == 3 else MarketKind.HEAD_TO_HEAD,
            start_time=start_time,
            outcomes=outcomes,
        )

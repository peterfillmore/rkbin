"""FastAPI application entrypoint.

Run with:  uvicorn app.main:app --reload
"""
from __future__ import annotations

import logging
from contextlib import asynccontextmanager

from fastapi import FastAPI

from .api import router
from .config import get_settings
from .poller import Poller
from .sources import build_sources
from .store import Store

logging.basicConfig(
    level=logging.INFO, format="%(asctime)s %(levelname)s %(name)s: %(message)s"
)


@asynccontextmanager
async def lifespan(app: FastAPI):
    settings = get_settings()
    store = Store(opportunity_ttl_seconds=settings.opportunity_ttl_seconds)
    poller = Poller(build_sources(settings), store, settings)
    app.state.store = store
    app.state.poller = poller
    if settings.poll_on_startup:
        await poller.start()
    yield
    await poller.shutdown()


app = FastAPI(
    title="Betting Arbitrage Detector",
    description=(
        "Polls the Betfair Exchange API and fixed-odds bookmakers (via "
        "The Odds API) in the background, matches markets that refer to the "
        "same event, and exposes detected arbitrage opportunities over REST."
    ),
    version="0.1.0",
    lifespan=lifespan,
)
app.include_router(router)

"""REST and WebSocket endpoints, plus the browser dashboard."""
from __future__ import annotations

import asyncio
from pathlib import Path

from fastapi import (
    APIRouter,
    HTTPException,
    Query,
    Request,
    WebSocket,
    WebSocketDisconnect,
)
from fastapi.responses import FileResponse, RedirectResponse
from pydantic import BaseModel, Field

from .models import MarketSnapshot, Opportunity, PollerStatus
from .poller import Poller
from .store import Store
from .ws import Broadcaster

router = APIRouter()

_STATIC_DIR = Path(__file__).parent / "static"


@router.get("/", include_in_schema=False)
async def root() -> RedirectResponse:
    return RedirectResponse(url="/dashboard")


@router.get("/dashboard", include_in_schema=False)
async def dashboard() -> FileResponse:
    return FileResponse(_STATIC_DIR / "dashboard.html", media_type="text/html")


def _store(request: Request) -> Store:
    return request.app.state.store


def _poller(request: Request) -> Poller:
    return request.app.state.poller


@router.get("/health")
async def health() -> dict:
    return {"status": "ok"}


@router.get("/status", response_model=PollerStatus)
async def status(request: Request) -> PollerStatus:
    return _poller(request).status


@router.get("/markets", response_model=list[MarketSnapshot])
async def list_markets(
    request: Request,
    source: str | None = Query(None, description="Filter by source, e.g. 'betfair'"),
    sport: str | None = Query(None, description="Filter by sport name"),
) -> list[MarketSnapshot]:
    return _store(request).markets(source=source, sport=sport)


@router.get("/markets/{source}/{market_id}", response_model=MarketSnapshot)
async def get_market(request: Request, source: str, market_id: str) -> MarketSnapshot:
    market = _store(request).market(f"{source}:{market_id}")
    if market is None:
        raise HTTPException(status_code=404, detail="Market not found")
    return market


@router.get("/opportunities", response_model=list[Opportunity])
async def list_opportunities(
    request: Request,
    active: bool = Query(True, description="Only opportunities seen recently"),
    min_profit: float | None = Query(
        None, ge=0, description="Minimum profit margin, e.g. 0.01 for 1%"
    ),
    sport: str | None = Query(None),
    kind: str | None = Query(
        None, pattern="^(cross_book|back_lay)$", description="Opportunity kind"
    ),
) -> list[Opportunity]:
    return _store(request).opportunities(
        active_only=active, min_profit=min_profit, sport=sport, kind=kind
    )


@router.get("/opportunities/{opp_id}", response_model=Opportunity)
async def get_opportunity(request: Request, opp_id: str) -> Opportunity:
    opp = _store(request).opportunity(opp_id)
    if opp is None:
        raise HTTPException(status_code=404, detail="Opportunity not found")
    return opp


@router.post("/poller/start", response_model=PollerStatus)
async def start_poller(request: Request) -> PollerStatus:
    poller = _poller(request)
    await poller.start()
    return poller.status


@router.post("/poller/stop", response_model=PollerStatus)
async def stop_poller(request: Request) -> PollerStatus:
    poller = _poller(request)
    await poller.stop()
    return poller.status


@router.post("/poller/poll", response_model=PollerStatus)
async def poll_now(request: Request) -> PollerStatus:
    """Run one poll immediately (also wakes the background loop if running)."""
    poller = _poller(request)
    if poller.status.running:
        poller.trigger()
    else:
        await poller.poll_once()
    return poller.status


async def _drain_incoming(websocket: WebSocket) -> None:
    """Consume (and ignore) client messages so disconnects are noticed."""
    while True:
        await websocket.receive_text()


async def _forward_broadcasts(queue: asyncio.Queue, websocket: WebSocket) -> None:
    while True:
        message = await queue.get()
        await websocket.send_json(message)


@router.websocket("/ws")
async def ws_opportunities(websocket: WebSocket) -> None:
    """Stream arbitrage opportunities.

    On connect the server sends a `snapshot` message with all currently
    active opportunities; after every background poll it sends a `poll`
    message containing the poller status plus `new`, `changed` (profit
    margin moved) and `expired` (ids no longer active) opportunities.
    """
    store: Store = websocket.app.state.store
    poller: Poller = websocket.app.state.poller
    broadcaster: Broadcaster = websocket.app.state.broadcaster
    await websocket.accept()
    queue = broadcaster.subscribe()
    tasks: set[asyncio.Task] = set()
    try:
        await websocket.send_json(
            {
                "type": "snapshot",
                "status": poller.status.model_dump(mode="json"),
                "opportunities": [
                    o.model_dump(mode="json")
                    for o in store.opportunities(active_only=True)
                ],
            }
        )
        tasks = {
            asyncio.create_task(_drain_incoming(websocket)),
            asyncio.create_task(_forward_broadcasts(queue, websocket)),
        }
        done, pending = await asyncio.wait(
            tasks, return_when=asyncio.FIRST_COMPLETED
        )
        for task in pending:
            task.cancel()
        for task in done:
            exc = task.exception()
            if exc is not None and not isinstance(exc, WebSocketDisconnect):
                raise exc
    except WebSocketDisconnect:
        pass
    finally:
        broadcaster.unsubscribe(queue)
        for task in tasks:
            task.cancel()


class PollerConfig(BaseModel):
    poll_interval_seconds: float | None = Field(None, gt=0.5, le=3600)


@router.patch("/poller/config", response_model=PollerStatus)
async def update_poller_config(request: Request, config: PollerConfig) -> PollerStatus:
    poller = _poller(request)
    if config.poll_interval_seconds is not None:
        poller.status.poll_interval_seconds = config.poll_interval_seconds
    return poller.status

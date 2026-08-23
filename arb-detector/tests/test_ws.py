import asyncio

import pytest
from fastapi.testclient import TestClient

from app import config
from app.main import app
from app.ws import Broadcaster


@pytest.fixture()
def client(monkeypatch):
    monkeypatch.setenv("ARB_ENABLED_SOURCES", "mock")
    monkeypatch.setenv("ARB_POLL_ON_STARTUP", "false")
    monkeypatch.setenv("ARB_MIN_PROFIT_MARGIN", "0.001")
    config.get_settings.cache_clear()
    with TestClient(app) as test_client:
        yield test_client
    config.get_settings.cache_clear()


def test_ws_snapshot_then_poll_updates(client):
    with client.websocket_connect("/ws") as ws:
        snapshot = ws.receive_json()
        assert snapshot["type"] == "snapshot"
        assert snapshot["opportunities"] == []  # nothing polled yet
        assert snapshot["status"]["polls_completed"] == 0

        client.post("/poller/poll")
        update = ws.receive_json()
        assert update["type"] == "poll"
        assert update["status"]["polls_completed"] == 1
        assert update["expired"] == []
        assert update["changed"] == []
        # The mock source reliably produces opportunities at a 0.1% floor.
        assert update["new"]
        for opp in update["new"]:
            assert opp["kind"] in ("cross_book", "back_lay")
            assert opp["profit_margin"] >= 0.001
            assert opp["legs"]

        # A second poll re-prices everything: every reported opportunity must
        # land in exactly one of new/changed, or be expired.
        first_ids = {o["id"] for o in update["new"]}
        client.post("/poller/poll")
        update2 = ws.receive_json()
        assert update2["type"] == "poll"
        assert update2["status"]["polls_completed"] == 2
        changed_ids = {o["id"] for o in update2["changed"]}
        new_ids = {o["id"] for o in update2["new"]}
        assert changed_ids <= first_ids
        assert not (new_ids & first_ids)


def test_ws_snapshot_includes_existing_opportunities(client):
    client.post("/poller/poll")
    with client.websocket_connect("/ws") as ws:
        snapshot = ws.receive_json()
        assert snapshot["type"] == "snapshot"
        assert snapshot["opportunities"]


def test_ws_multiple_clients_all_receive(client):
    with client.websocket_connect("/ws") as ws1, client.websocket_connect(
        "/ws"
    ) as ws2:
        ws1.receive_json()
        ws2.receive_json()
        client.post("/poller/poll")
        assert ws1.receive_json()["type"] == "poll"
        assert ws2.receive_json()["type"] == "poll"


def test_ws_disconnect_unsubscribes(client):
    broadcaster = client.app.state.broadcaster
    with client.websocket_connect("/ws") as ws:
        ws.receive_json()
        assert broadcaster.subscriber_count == 1
    assert broadcaster.subscriber_count == 0


def test_broadcaster_drops_oldest_when_queue_full():
    broadcaster = Broadcaster()
    queue = broadcaster.subscribe()
    for i in range(queue.maxsize + 5):
        broadcaster.publish({"n": i})
    assert queue.qsize() == queue.maxsize
    # The oldest messages were dropped; the newest survived.
    first = queue.get_nowait()
    assert first["n"] == 5
    rest = []
    while not queue.empty():
        rest.append(queue.get_nowait()["n"])
    assert rest[-1] == queue.maxsize + 4


def test_broadcaster_publish_without_subscribers_is_noop():
    Broadcaster().publish({"type": "poll"})  # must not raise

import pytest
from fastapi.testclient import TestClient

from app import config
from app.main import app


@pytest.fixture()
def client(monkeypatch):
    monkeypatch.setenv("ARB_ENABLED_SOURCES", "mock")
    monkeypatch.setenv("ARB_POLL_ON_STARTUP", "false")
    monkeypatch.setenv("ARB_MIN_PROFIT_MARGIN", "0.001")
    config.get_settings.cache_clear()
    with TestClient(app) as test_client:
        yield test_client
    config.get_settings.cache_clear()


def test_health(client):
    resp = client.get("/health")
    assert resp.status_code == 200
    assert resp.json() == {"status": "ok"}


def test_status_reports_poller_state(client):
    resp = client.get("/status")
    assert resp.status_code == 200
    body = resp.json()
    assert body["running"] is False
    assert body["sources"] == ["mock"]


def test_poll_populates_markets_and_opportunities(client):
    resp = client.post("/poller/poll")
    assert resp.status_code == 200
    assert resp.json()["polls_completed"] == 1
    assert resp.json()["markets_tracked"] > 0

    markets = client.get("/markets").json()
    assert markets
    sources = {m["source"] for m in markets}
    assert {"mock_exchange", "mock_books"} <= sources

    one = markets[0]
    resp = client.get(f"/markets/{one['source']}/{one['market_id']}")
    assert resp.status_code == 200
    assert resp.json()["event_name"] == one["event_name"]

    resp = client.get("/opportunities")
    assert resp.status_code == 200
    for opp in resp.json():
        assert opp["profit_margin"] >= 0.001
        assert opp["kind"] in ("cross_book", "back_lay")
        detail = client.get(f"/opportunities/{opp['id']}")
        assert detail.status_code == 200


def test_market_and_opportunity_404(client):
    assert client.get("/markets/nope/xyz").status_code == 404
    assert client.get("/opportunities/doesnotexist").status_code == 404


def test_opportunity_filters_validate(client):
    assert client.get("/opportunities", params={"kind": "bogus"}).status_code == 422
    assert client.get("/opportunities", params={"min_profit": -1}).status_code == 422


def test_poller_start_stop_and_config(client):
    resp = client.post("/poller/start")
    assert resp.status_code == 200
    assert resp.json()["running"] is True

    resp = client.patch("/poller/config", json={"poll_interval_seconds": 5})
    assert resp.status_code == 200
    assert resp.json()["poll_interval_seconds"] == 5

    resp = client.post("/poller/stop")
    assert resp.status_code == 200
    assert resp.json()["running"] is False


def test_dashboard_served(client):
    resp = client.get("/dashboard")
    assert resp.status_code == 200
    assert "text/html" in resp.headers["content-type"]
    assert "Arbitrage monitor" in resp.text

    resp = client.get("/", follow_redirects=False)
    assert resp.status_code == 307
    assert resp.headers["location"] == "/dashboard"

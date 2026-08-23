"""Application settings, loaded from environment variables / .env file."""
from functools import lru_cache

from pydantic_settings import BaseSettings, SettingsConfigDict


class Settings(BaseSettings):
    model_config = SettingsConfigDict(
        env_prefix="ARB_", env_file=".env", extra="ignore"
    )

    # Polling / detection
    poll_interval_seconds: float = 30.0
    min_profit_margin: float = 0.005
    betfair_commission: float = 0.05
    poll_on_startup: bool = True
    # How long (seconds) an opportunity stays listed after it was last seen.
    opportunity_ttl_seconds: float = 120.0
    # Minimum name-similarity score (0..1) to treat two events as the same.
    event_match_threshold: float = 0.75
    # Maximum start-time difference (minutes) for two events to be matched.
    event_match_max_start_diff_minutes: float = 30.0

    enabled_sources: str = "mock"

    # Betfair
    betfair_app_key: str = ""
    betfair_username: str = ""
    betfair_password: str = ""
    betfair_cert_file: str = ""
    betfair_key_file: str = ""
    betfair_event_type_ids: str = "1,2"
    betfair_max_markets: int = 50

    # The Odds API
    odds_api_key: str = ""
    odds_api_sports: str = "soccer_epl"
    odds_api_regions: str = "au,uk,eu"

    @property
    def enabled_source_names(self) -> list[str]:
        return [s.strip().lower() for s in self.enabled_sources.split(",") if s.strip()]


@lru_cache
def get_settings() -> Settings:
    return Settings()

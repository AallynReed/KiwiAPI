"""Master-flippable feature toggles.

A whole site feature (the Mods Hub, the Market) can be hidden completely without
a code change or restart - it's a ``bool`` in the runtime-config registry
(``app/admin/runtime_config.py``), flippable from the dev-portal Configuration
tab. When OFF:
  - the navbar entry is hidden (a Jinja context flag the site router injects),
  - the page routes + ``/site/<feature>/*`` proxies 404 (gated in app/site/router.py),
  - the ``/v1/<feature>/*`` (and git) routers 404 via the dependencies here.
Stored data is untouched and the feature reappears intact when toggled back ON.
"""

from app.admin import runtime_config
from app.core.errors import APIError, ErrorCode

MODS_HUB_FLAG = "feature_mods_hub_enabled"
MARKET_FLAG = "feature_market_enabled"


async def is_enabled(flag: str) -> bool:
    """Resolved value of a feature flag (runtime override, else default)."""
    return bool(await runtime_config.get_setting(flag))


def _disabled(name: str) -> APIError:
    # 404 (not 403/503) so a disabled feature is indistinguishable from one that
    # never existed - it's "hidden", not "forbidden".
    return APIError(404, ErrorCode.not_found, f"The {name} is currently disabled.")


async def require_mods_hub_enabled() -> None:
    if not await is_enabled(MODS_HUB_FLAG):
        raise _disabled("Mods Hub")


async def require_market_enabled() -> None:
    if not await is_enabled(MARKET_FLAG):
        raise _disabled("Market")

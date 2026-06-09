"""Per-endpoint rate-limit overrides.

Every API-token request is already throttled globally (``api_rate_limit_*``).
Some routes are heavier than others, so this registry lets specific routes carry
an *additional*, tighter budget - keyed by the FastAPI route template so no
per-router wiring is needed (``get_token_context`` consults it centrally).

To add a limit for a real endpoint later, register its route template here, e.g.

    register_endpoint_limit("/v1/reports/generate", max_requests=10, window_seconds=60)
"""

# route template -> (max_requests, window_seconds)
# Empty in the base: add entries here for heavy endpoints as they're built.
_ENDPOINT_LIMITS: dict[str, tuple[int, int]] = {}


def register_endpoint_limit(route: str, max_requests: int, window_seconds: int) -> None:
    _ENDPOINT_LIMITS[route] = (max_requests, window_seconds)


def endpoint_limit_for(route: str | None) -> tuple[int, int] | None:
    if route is None:
        return None
    return _ENDPOINT_LIMITS.get(route)

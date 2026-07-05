"""Per-endpoint rate-limit overrides on top of the global per-token cap.

Keyed by the FastAPI route template so no per-router wiring is needed
(``get_token_context`` consults it centrally). Empty in the base.
"""

# route template -> (max_requests, window_seconds)
_ENDPOINT_LIMITS: dict[str, tuple[int, int]] = {}


def register_endpoint_limit(route: str, max_requests: int, window_seconds: int) -> None:
    _ENDPOINT_LIMITS[route] = (max_requests, window_seconds)


def endpoint_limit_for(route: str | None) -> tuple[int, int] | None:
    if route is None:
        return None
    return _ENDPOINT_LIMITS.get(route)

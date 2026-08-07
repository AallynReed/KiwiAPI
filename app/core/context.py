"""Per-request context (request id), shared across middleware, logging, and the
error envelope. Lives in its own low-level module so both `observability` and
`errors` can import it without a cycle."""

import contextvars
import secrets

_request_id: contextvars.ContextVar[str] = contextvars.ContextVar("request_id", default="-")


def new_request_id() -> str:
    return "req_" + secrets.token_hex(8)


def set_request_id(value: str):
    """Bind a request id to the current context. Returns the token to reset with."""
    return _request_id.set(value)


def reset_request_id(token) -> None:
    _request_id.reset(token)


def get_request_id() -> str:
    """The current request's id, or '-' outside a request."""
    return _request_id.get()

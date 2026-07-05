"""Discord interactions webhook (``/discord/interactions``).

Discord POSTs every interaction (slash command, button, modal, …) to a single
URL set as the app's "Interactions Endpoint URL". Each request is signed with the
app's Ed25519 key and we MUST verify it (replying ``401`` on failure) or Discord
refuses the endpoint - it even probes with a deliberately bad signature when you
save the URL. The first real message is a PING (type 1); we reply PONG (type 1).

Verification: the signed message is ``timestamp + raw_body``; the signature and
timestamp arrive in ``X-Signature-Ed25519`` / ``X-Signature-Timestamp``. The
public key (hex) is ``settings.discord_public_key``. No new dependency - the
already-required ``cryptography`` package does Ed25519.

Non-PING interactions dispatch on the command name in ``commands.handle``.
"""
import json
import logging

from cryptography.exceptions import InvalidSignature
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey
from fastapi import APIRouter, Request
from fastapi.responses import JSONResponse

from app.core.config import settings
from app.discord import commands

logger = logging.getLogger("kiwi.discord")

router = APIRouter(prefix="/discord", tags=["discord"])

# Interaction PING / response PONG type ids (Discord API). Command dispatch and
# its response types live in commands.py.
_TYPE_PING = 1
_RESP_PONG = 1


def _verify(signature_hex: str, timestamp: str, body: bytes) -> bool:
    """True iff ``signature_hex`` is a valid Ed25519 signature of
    ``timestamp + body`` under the app's public key."""
    key = settings.discord_public_key
    if not key:
        return False
    try:
        Ed25519PublicKey.from_public_bytes(bytes.fromhex(key)).verify(
            bytes.fromhex(signature_hex), timestamp.encode() + body,
        )
        return True
    except (InvalidSignature, ValueError):
        return False


@router.post("/interactions", include_in_schema=False)
async def interactions(request: Request) -> JSONResponse:
    sig = request.headers.get("X-Signature-Ed25519")
    ts = request.headers.get("X-Signature-Timestamp")
    body = await request.body()
    # Discord REQUIRES 401 on a missing/invalid signature.
    if not sig or not ts or not _verify(sig, ts, body):
        return JSONResponse({"error": "invalid request signature"}, status_code=401)

    try:
        data = json.loads(body or b"{}")
    except json.JSONDecodeError:
        return JSONResponse({"error": "invalid payload"}, status_code=400)

    if data.get("type") == _TYPE_PING:
        return JSONResponse({"type": _RESP_PONG})
    return JSONResponse(await commands.handle(data))

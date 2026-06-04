"""GitHub secret-scanning request signature verification.

GitHub signs each secret-scanning webhook with an ECDSA key and sends the
key identifier + base64 signature in headers. We verify the signature over the
*raw* request body against GitHub's published public keys (cached in-process,
refreshed when an unknown key identifier appears).

Docs: https://docs.github.com/code-security/secret-scanning/secret-scanning-partner-program
"""

import base64
import logging

import httpx
from cryptography.exceptions import InvalidSignature
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec

from app.core.config import settings

logger = logging.getLogger("kiwi.scanning")

KEY_ID_HEADER = "Github-Public-Key-Identifier"
SIGNATURE_HEADER = "Github-Public-Key-Signature"

_key_cache: dict[str, str] = {}


async def _fetch_keys() -> dict[str, str]:
    async with httpx.AsyncClient(timeout=10) as client:
        resp = await client.get(settings.github_secret_scanning_keys_url)
        resp.raise_for_status()
        data = resp.json()
    return {k["key_identifier"]: k["key"] for k in data.get("public_keys", [])}


async def _get_key(key_id: str) -> str | None:
    if key_id in _key_cache:
        return _key_cache[key_id]
    try:
        _key_cache.update(await _fetch_keys())
    except (httpx.HTTPError, ValueError, KeyError):
        logger.exception("Failed to fetch GitHub secret-scanning public keys")
        return None
    return _key_cache.get(key_id)


async def verify_signature(body: bytes, key_id: str | None, signature_b64: str | None) -> bool:
    """True if ``signature_b64`` is a valid GitHub ECDSA signature over ``body``."""
    if not key_id or not signature_b64:
        return False
    pem = await _get_key(key_id)
    if pem is None:
        return False
    try:
        public_key = serialization.load_pem_public_key(pem.encode())
        if not isinstance(public_key, ec.EllipticCurvePublicKey):
            return False
        public_key.verify(
            base64.b64decode(signature_b64), body, ec.ECDSA(hashes.SHA256())
        )
        return True
    except (InvalidSignature, ValueError, TypeError):
        return False

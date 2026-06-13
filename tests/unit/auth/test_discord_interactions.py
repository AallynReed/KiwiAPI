"""Unit tests for the Discord interactions Ed25519 signature verification.

Discord refuses the interactions endpoint unless it verifies the request
signature (and replies 401 on a bad one), so this is the security-critical bit.
"""
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

from app.core.config import settings
from app.discord import router as dr


def _fresh_key() -> Ed25519PrivateKey:
    priv = Ed25519PrivateKey.generate()
    settings.discord_public_key = priv.public_key().public_bytes(
        serialization.Encoding.Raw, serialization.PublicFormat.Raw,
    ).hex()
    return priv


def test_valid_signature_passes():
    priv = _fresh_key()
    ts, body = "1700000000", b'{"type":1}'
    sig = priv.sign(ts.encode() + body).hex()
    assert dr._verify(sig, ts, body) is True


def test_tampered_body_fails():
    priv = _fresh_key()
    ts, body = "1700000000", b'{"type":1}'
    sig = priv.sign(ts.encode() + body).hex()
    assert dr._verify(sig, ts, b'{"type":2}') is False


def test_forged_signature_fails():
    _fresh_key()
    assert dr._verify("00" * 64, "1700000000", b'{"type":1}') is False


def test_garbage_signature_hex_fails():
    _fresh_key()
    # Non-hex / wrong-length signature must be rejected, not raise.
    assert dr._verify("not-hex", "1700000000", b'{}') is False


def test_no_public_key_configured_fails():
    settings.discord_public_key = None
    assert dr._verify("00" * 64, "1700000000", b'{}') is False

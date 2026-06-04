import base64

from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec

from app.scanning import github


async def test_verify_rejects_missing_headers():
    assert await github.verify_signature(b"body", None, None) is False
    assert await github.verify_signature(b"body", "id", None) is False
    assert await github.verify_signature(b"body", None, "sig") is False


async def test_verify_valid_and_tampered(monkeypatch):
    # Generate an EC keypair, preload the public key in the cache (no network),
    # sign a body, and confirm a good signature verifies and a tampered one fails.
    async def _no_fetch():
        return {}

    monkeypatch.setattr(github, "_fetch_keys", _no_fetch)  # never hit the network

    priv = ec.generate_private_key(ec.SECP256R1())
    pub_pem = priv.public_key().public_bytes(
        serialization.Encoding.PEM, serialization.PublicFormat.SubjectPublicKeyInfo
    ).decode()
    monkeypatch.setitem(github._key_cache, "key-1", pub_pem)

    body = b'[{"token":"kiwi_abc"}]'
    sig_b64 = base64.b64encode(priv.sign(body, ec.ECDSA(hashes.SHA256()))).decode()

    assert await github.verify_signature(body, "key-1", sig_b64) is True
    assert await github.verify_signature(b"tampered body", "key-1", sig_b64) is False
    assert await github.verify_signature(body, "unknown-key", sig_b64) is False

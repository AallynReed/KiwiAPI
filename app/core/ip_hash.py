"""Hashing for API-token IP allowlists.

We store HMAC-SHA256(per-token salt, normalized IP) - same shape as password
verifiers: an attacker (or admin) with full DB access can VERIFY a guessed
IP against the hash, but can't trivially enumerate or recover the IPs the
token's owner pinned. Per-token salts mean two tokens that pin the same IP
have different hashes (no cross-token correlation).

Trade-offs deliberately made:

- **CIDRs are not supported.** A hash can't range-match a single IP, and
  expanding e.g. /16 into 65k hashes leaks the prefix to anyone who counts.
  The validator at ``app/tokens/router.py`` rejects anything containing a
  ``/``. Users with dynamic IPs should either omit the allowlist (the field
  is opt-in) or pin specific addresses.

- **IPv4 brute-force is feasible** for an attacker with full DB access.
  The address space is only ~4 billion; HMAC-SHA256 on commodity hardware
  scans it in minutes. The point of hashing here is parity with password
  storage (no plaintext PII in the DB; admin can't read IPs by looking),
  not computational impossibility. For higher assurance, pair this with
  short token lifetimes + rotation.
"""

from __future__ import annotations

import hashlib
import hmac
import ipaddress
import secrets

# 16 bytes = 128 bits of entropy in the salt. Encoded urlsafe-base64 (no padding)
# so it round-trips cleanly through JSON / env / Mongo.
_SALT_BYTES = 16


def make_ip_salt() -> str:
    """Generate a fresh per-token salt. Call once at token creation."""
    return secrets.token_urlsafe(_SALT_BYTES)


def normalize_ip(value: str) -> str:
    """Canonicalize an IP string (e.g. collapse IPv6 ``0:0:…:1`` → ``::1``).

    Raises ``ValueError`` for invalid IPs and for any input containing ``/``
    (CIDRs are not supported - see module docstring).
    """
    value = value.strip()
    if "/" in value:
        raise ValueError(
            "CIDR ranges aren't supported on token IP allowlists "
            "(hashes can't range-match); pin specific addresses instead."
        )
    return str(ipaddress.ip_address(value))


def hash_ip(salt: str, ip: str) -> str:
    """Hex digest of HMAC-SHA256(salt, normalized-ip). Constant-time-safe to
    compare with ``hmac.compare_digest`` if the caller has a guess in hand -
    most call sites just check ``hash_ip(salt, x) in allowed_hashes``."""
    return hmac.new(salt.encode("ascii"), ip.encode("ascii"), hashlib.sha256).hexdigest()


def ip_allowed(client_ip: str, salt: str | None, allowed_hashes: list[str]) -> bool:
    """True iff hashing ``client_ip`` with this token's salt lands in the
    allowed list. Falsy salt or empty client IP → False (the caller already
    checked the list is non-empty before calling, so a missing salt at that
    point means a bug - defensive False rather than crashing the request)."""
    if not salt or not client_ip:
        return False
    try:
        normalized = normalize_ip(client_ip)
    except ValueError:
        return False
    candidate = hash_ip(salt, normalized)
    # Linear scan with constant-time compare per element. The allowed list is
    # tiny (a handful of pinned IPs in practice) so this is fast enough.
    return any(hmac.compare_digest(candidate, h) for h in allowed_hashes)

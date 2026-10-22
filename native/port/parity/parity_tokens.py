"""Mint or revoke the temporary API tokens parity runs send.

Runs inside api-py (PYTHONPATH=/app). `mint` prints shell exports for
KIWI_PARITY_TOKEN (all scopes) and KIWI_PARITY_TOKEN_NO_GEMS (every scope but
gems:read), both owned by the admin account and expiring in a day, plus
KIWI_PARITY_JWT, a 12h portal access token for the same account, and
KIWI_PARITY_ADMIN_ID, that account's id. `revoke` deletes every token named
"parity run".
"""

import asyncio
import sys
from datetime import timedelta

from beanie import init_beanie
from pymongo import AsyncMongoClient

from app.auth.models import User
from app.core.config import settings
from app.core.ip_hash import make_ip_salt
from app.core.scopes import _KNOWN_MASK, SCOPE_BITS
from app.core.security import create_access_token, generate_api_token
from app.core.utils import utcnow
from app.tokens.models import ApiToken

NAME = "parity run"


async def main(action: str) -> None:
    client = AsyncMongoClient(settings.mongo_uri, tz_aware=True)
    await init_beanie(database=client[settings.mongo_db], document_models=[User, ApiToken])
    if action == "revoke":
        res = await ApiToken.find(ApiToken.name == NAME).delete()
        print(f"deleted {res.deleted_count if res else 0} token(s)", file=sys.stderr)
        return
    admin = await User.find_one(User.email == settings.admin_email, User.is_superuser == True)  # noqa: E712
    if admin is None:
        sys.exit("no superuser with ADMIN_EMAIL")
    for var, mask in (("KIWI_PARITY_TOKEN", 0), ("KIWI_PARITY_TOKEN_NO_GEMS", _KNOWN_MASK & ~SCOPE_BITS["gems:read"])):
        full, hashed, prefix = generate_api_token()
        await ApiToken(
            user_id=admin.id, name=NAME, prefix=prefix, hashed_token=hashed, scopes=mask,
            ip_salt=make_ip_salt(), expires_at=utcnow() + timedelta(days=1),
        ).insert()
        print(f"export {var}={full}")
    jwt = create_access_token(str(admin.id), admin.token_version, expires_delta=timedelta(hours=12))
    print(f"export KIWI_PARITY_JWT={jwt}")
    print(f"export KIWI_PARITY_ADMIN_ID={admin.id}")


if __name__ == "__main__":
    asyncio.run(main(sys.argv[1] if len(sys.argv) > 1 else "mint"))

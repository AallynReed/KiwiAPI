import logging

from app.auth.models import User
from app.core.config import settings
from app.core.security import hash_password, verify_password
from app.core.utils import utcnow

logger = logging.getLogger("kiwi.bootstrap")


async def bootstrap_admin() -> None:
    """Ensure the configured admin account exists, is a superuser, and matches
    ADMIN_PASSWORD.

    Idempotent and treats `.env` as the source of truth for the admin account:
    creates it if missing, promotes it to superuser, and resets its password to
    ADMIN_PASSWORD if they differ. No-op if ADMIN_EMAIL / ADMIN_PASSWORD aren't
    both set.

    Note: this means the admin password is managed via ADMIN_PASSWORD, not the
    portal - a portal-side change would be reset to ADMIN_PASSWORD on next boot.
    """
    if not settings.admin_email or not settings.admin_password:
        return

    email = settings.admin_email.lower()
    user = await User.find_one(User.email == email)

    if user is None:
        user = User(
            email=email,
            hashed_password=hash_password(settings.admin_password),
            display_name="Administrator",
            is_superuser=True,
            is_verified=True,
        )
        await user.insert()
        logger.warning("Bootstrapped admin account %s", email)
        return

    changed = False
    if not user.is_superuser:
        user.is_superuser = True
        changed = True
        logger.warning("Promoted existing account %s to superuser", email)

    # Keep the admin password in sync with ADMIN_PASSWORD (env is authoritative).
    if not verify_password(settings.admin_password, user.hashed_password):
        user.hashed_password = hash_password(settings.admin_password)
        changed = True
        logger.warning("Reset admin password for %s to match ADMIN_PASSWORD", email)

    if changed:
        user.updated_at = utcnow()
        await user.save()

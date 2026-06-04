from datetime import datetime, timezone


def utcnow() -> datetime:
    """Timezone-aware UTC now — use everywhere instead of datetime.utcnow()."""
    return datetime.now(timezone.utc)

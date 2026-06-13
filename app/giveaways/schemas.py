"""Request + response models for the giveaways feature."""
from datetime import datetime, timezone

from pydantic import BaseModel, Field, field_validator

from app.giveaways.models import CodeStatus, GiveawayStatus


def _as_utc(v: datetime) -> datetime:
    """Coerce a parsed datetime to tz-aware UTC. The admin form sends an ISO
    string with a 'Z' (we convert in JS), but a naive value is treated as UTC
    rather than blowing up later comparisons against ``utcnow()``."""
    if v.tzinfo is None:
        return v.replace(tzinfo=timezone.utc)
    return v.astimezone(timezone.utc)


# ── Vault items (drawers) ───────────────────────────────────────────────────

class VaultItemCreate(BaseModel):
    name: str = Field(min_length=1, max_length=120)
    description: str | None = Field(default=None, max_length=2000)


class VaultItemUpdate(BaseModel):
    name: str | None = Field(default=None, min_length=1, max_length=120)
    description: str | None = Field(default=None, max_length=2000)


class VaultItemView(BaseModel):
    id: str
    name: str
    description: str | None = None
    # Code tallies by status, so the drawer shows "42 available / 3 reserved".
    available: int = 0
    reserved: int = 0
    awarded: int = 0
    total: int = 0
    created_at: datetime


# ── Vault codes (inside a drawer) ───────────────────────────────────────────

class VaultCodesAdd(BaseModel):
    """Drop one or many codes into a drawer at once (the form pastes one per
    line). Blanks/dupes are cleaned server-side."""
    codes: list[str] = Field(min_length=1)


class VaultCodeUpdate(BaseModel):
    code: str = Field(min_length=1, max_length=400)


class VaultCodeView(BaseModel):
    id: str
    code: str                      # master-only surface, so the code is shown
    status: CodeStatus
    giveaway_id: str | None = None
    awarded_to_email: str | None = None
    awarded_at: datetime | None = None
    created_at: datetime


# ── Giveaways (admin) ───────────────────────────────────────────────────────

class GiveawayCreate(BaseModel):
    title: str = Field(min_length=1, max_length=160)
    description: str | None = Field(default=None, max_length=4000)
    starts_at: datetime
    ends_at: datetime
    vault_item_id: str             # the drawer to reserve a prize code from

    @field_validator("starts_at", "ends_at")
    @classmethod
    def _utc(cls, v: datetime) -> datetime:
        return _as_utc(v)


class GiveawayUpdate(BaseModel):
    title: str | None = Field(default=None, min_length=1, max_length=160)
    description: str | None = Field(default=None, max_length=4000)
    starts_at: datetime | None = None
    ends_at: datetime | None = None
    vault_item_id: str | None = None

    @field_validator("starts_at", "ends_at")
    @classmethod
    def _utc(cls, v: datetime | None) -> datetime | None:
        return _as_utc(v) if v is not None else v


class GiveawayAdminView(BaseModel):
    id: str
    title: str
    description: str | None = None
    prize_name: str
    status: GiveawayStatus
    starts_at: datetime
    ends_at: datetime
    entry_count: int
    vault_item_id: str | None = None
    vault_item_name: str | None = None
    prize_code_id: str | None = None
    winner_user_id: str | None = None
    winner_username: str | None = None
    winner_email: str | None = None
    drawn_at: datetime | None = None
    created_at: datetime


# ── Giveaways (public) ──────────────────────────────────────────────────────

class GiveawayPublicView(BaseModel):
    id: str
    title: str
    description: str | None = None
    prize_name: str
    status: GiveawayStatus
    starts_at: datetime
    ends_at: datetime
    entry_count: int
    winner_username: str | None = None   # public recognition; code is NEVER here


class EnterResponse(BaseModel):
    giveaway_id: str
    entered: bool
    entry_count: int


class MyGiveawayView(BaseModel):
    """A giveaway the signed-in user entered. ``code`` is present only when they
    WON (and only ever returned to that winner) so they can retrieve it from the
    dashboard at any time, not just from the email."""
    giveaway_id: str
    title: str
    prize_name: str
    status: GiveawayStatus
    starts_at: datetime
    ends_at: datetime
    entered_at: datetime
    won: bool
    code: str | None = None

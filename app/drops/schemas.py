"""Request + response models for file drops."""
from datetime import datetime

from pydantic import BaseModel, Field, field_validator

_PIN_CHARS = set("0123456789")


class DropCreate(BaseModel):
    """The admin form. Everything has a sane default except the label."""

    label: str = Field(min_length=1, max_length=120)
    pin: str = Field(min_length=4, max_length=12)
    max_uploads: int = Field(default=1, ge=1, le=100)
    expires_in_hours: int = Field(default=24, ge=1, le=24 * 90)
    max_file_mb: int = Field(default=256, ge=1, le=2048)

    @field_validator("pin")
    @classmethod
    def _digits_only(cls, v: str) -> str:
        # Digits only: the PIN gets typed on a phone keypad as often as a
        # keyboard, and "was that an l or a 1" is not a puzzle worth shipping.
        if not v or set(v) - _PIN_CHARS:
            raise ValueError("The PIN must be digits only.")
        return v


class DropUpdate(BaseModel):
    """Edits to a live drop. Only what's safe to change mid-flight - the PIN and
    the slug are fixed once they've been handed out."""

    label: str | None = Field(default=None, min_length=1, max_length=120)
    max_uploads: int | None = Field(default=None, ge=1, le=100)
    extend_hours: int | None = Field(default=None, ge=1, le=24 * 90)
    revoked: bool | None = None


class DropUploadView(BaseModel):
    id: str
    filename: str
    size: int
    content_type: str | None = None
    sha256: str
    note: str | None = None
    uploaded_at: datetime


class DropView(BaseModel):
    """Master-facing view of a drop. The PIN is NOT here - it is hashed, and the
    plaintext only ever appears in the create response."""

    id: str
    slug: str
    url: str
    label: str
    max_uploads: int
    upload_count: int
    max_file_bytes: int
    expires_at: datetime
    revoked: bool
    open: bool                      # accepting uploads right now
    created_at: datetime
    uploads: list[DropUploadView] = []


class DropCreated(DropView):
    """The create response - the one and only time the PIN is readable."""

    pin: str


class DropPublicView(BaseModel):
    """What the uploader's page is told before they've entered anything. Enough
    to render the form and set expectations, and nothing about who made it."""

    label: str
    max_file_bytes: int
    uploads_left: int
    expires_at: datetime

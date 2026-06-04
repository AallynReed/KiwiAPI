from pydantic import BaseModel


class SecretScanMatch(BaseModel):
    """One leaked-token candidate, as posted by a secret-scanning partner."""

    token: str
    type: str | None = None
    url: str | None = None
    source: str | None = None


class SecretScanResult(BaseModel):
    """Per-token verdict, in the shape GitHub's partner program expects."""

    token_raw: str
    token_type: str
    label: str  # "true_positive" (real, revoked) or "false_positive" (not ours)

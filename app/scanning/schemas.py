from pydantic import BaseModel


class SecretScanResult(BaseModel):
    """Per-token verdict, in the shape GitHub's partner program expects."""

    token_raw: str
    token_type: str
    label: str  # "true_positive" (real, revoked) or "false_positive" (not ours)

"""Secret-scanning partner webhook.

When a Kiwi API token (``kiwi_<body>_<crc>``) is pushed to a public repository,
a partner like GitHub detects it by its published regex and POSTs it here. We
verify the request signature, confirm the token is really ours via its checksum +
hash, and auto-revoke it - closing the window between a leak and an attacker.
"""

import json
import logging

from fastapi import APIRouter, BackgroundTasks, Request

from app.auth.emails import send_token_compromised_email
from app.auth.models import User
from app.core.config import settings
from app.core.errors import APIError, ErrorCode
from app.core.security import hash_token, verify_token_checksum
from app.core.utils import utcnow
from app.scanning.github import KEY_ID_HEADER, SIGNATURE_HEADER, verify_signature
from app.scanning.schemas import SecretScanResult
from app.tokens.models import ApiToken

logger = logging.getLogger("kiwi.scanning")

router = APIRouter(prefix="/secret-scanning", tags=["secret-scanning"])

_REVOKE_REASON = "Exposed publicly (detected by secret scanning)"


@router.post("/github", response_model=list[SecretScanResult])
async def github_secret_scanning(
    request: Request, background_tasks: BackgroundTasks
) -> list[SecretScanResult]:
    """Receive leaked-token reports from GitHub secret scanning and auto-revoke.

    Returns one verdict per token (`true_positive` = real token we revoked,
    `false_positive` = not one of ours), as the partner program expects.
    """
    raw = await request.body()

    if settings.github_secret_scanning_verify:
        ok = await verify_signature(
            raw, request.headers.get(KEY_ID_HEADER), request.headers.get(SIGNATURE_HEADER)
        )
        if not ok:
            raise APIError(
                status_code=401,
                code=ErrorCode.not_authenticated,
                message="Invalid or missing secret-scanning signature",
            )

    try:
        matches = json.loads(raw)
    except json.JSONDecodeError:
        raise APIError(status_code=400, code=ErrorCode.bad_request, message="Body must be JSON")
    if not isinstance(matches, list):
        raise APIError(
            status_code=400,
            code=ErrorCode.bad_request,
            message="Body must be a JSON array of token matches",
        )

    results: list[SecretScanResult] = []
    for match in matches:
        if not isinstance(match, dict) or not match.get("token"):
            continue
        token = str(match["token"])
        token_type = str(match.get("type") or "kiwi_api_token")
        label = await _evaluate_and_revoke(token, background_tasks)
        results.append(SecretScanResult(token_raw=token, token_type=token_type, label=label))
    return results


async def _evaluate_and_revoke(token: str, background_tasks: BackgroundTasks) -> str:
    # A failed checksum means it can't be one of our tokens - cheap reject.
    if verify_token_checksum(token) is False:
        return "false_positive"

    doc = await ApiToken.find_one(ApiToken.hashed_token == hash_token(token))
    if doc is None:
        return "false_positive"

    if not doc.revoked:
        doc.revoked = True
        doc.revoked_at = utcnow()
        doc.revoke_reason = _REVOKE_REASON
        await doc.save()
        logger.warning("Auto-revoked leaked token %s (%s)", doc.id, doc.prefix)
        user = await User.get(doc.user_id)
        if user is not None and settings.security_email_notifications:
            background_tasks.add_task(send_token_compromised_email, user, doc.name, doc.prefix)
    return "true_positive"

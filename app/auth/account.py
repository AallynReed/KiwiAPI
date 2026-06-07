from fastapi import APIRouter, Depends, status
from fastapi.encoders import jsonable_encoder
from fastapi.responses import JSONResponse

from app.auth.models import Session, User
from app.auth.schemas import DeleteAccountRequest
from app.core.dependencies import get_current_user
from app.core.errors import APIError, ErrorCode
from app.core.utils import utcnow
from app.tokens.models import ApiToken
from app.usage.models import UsageEvent

router = APIRouter(prefix="/auth", tags=["auth"])


@router.get("/me/export")
async def export_account(user: User = Depends(get_current_user)) -> JSONResponse:
    """Download all of the account's data as JSON (GDPR portability)."""
    tokens = await ApiToken.find(ApiToken.user_id == user.id).to_list()
    sessions = await Session.find(Session.user_id == user.id).to_list()
    usage = await UsageEvent.find(UsageEvent.user_id == user.id).to_list()

    data = {
        "exported_at": utcnow(),
        "user": {
            "id": str(user.id), "email": user.email, "display_name": user.display_name,
            "is_verified": user.is_verified, "is_superuser": user.is_superuser,
            "roles": user.roles, "github_linked": user.github_id is not None,
            "created_at": user.created_at, "last_login_at": user.last_login_at,
        },
        "api_tokens": [
            {
                "id": str(t.id), "name": t.name, "prefix": t.prefix, "scopes": t.scopes,
                # Pinned IPs are hashed — only the count is exposed.
                "allowed_ip_count": len(t.allowed_ip_hashes),
                "revoked": t.revoked, "created_at": t.created_at,
                "last_used_at": t.last_used_at, "expires_at": t.expires_at,
                "request_count": t.request_count,
            }
            for t in tokens
        ],
        "sessions": [
            {
                "id": str(s.id), "ip": s.ip, "user_agent": s.user_agent, "revoked": s.revoked,
                "created_at": s.created_at, "last_used_at": s.last_used_at, "expires_at": s.expires_at,
            }
            for s in sessions
        ],
        "usage_events": [
            {
                "method": e.method, "route": e.route, "path": e.path,
                "status_code": e.status_code, "duration_ms": e.duration_ms, "created_at": e.created_at,
            }
            for e in usage
        ],
    }
    return JSONResponse(
        content=jsonable_encoder(data),
        headers={"Content-Disposition": 'attachment; filename="kiwi-export.json"'},
    )


@router.post("/delete-account", status_code=status.HTTP_204_NO_CONTENT)
async def delete_account(
    payload: DeleteAccountRequest, user: User = Depends(get_current_user)
) -> None:
    """Permanently delete the account and all associated data."""
    if payload.confirm_email.lower() != user.email.lower():
        raise APIError(
            status_code=400,
            code=ErrorCode.bad_request,
            message="The confirmation email doesn't match your account",
        )
    await ApiToken.find(ApiToken.user_id == user.id).delete()
    await Session.find(Session.user_id == user.id).delete()
    await UsageEvent.find(UsageEvent.user_id == user.id).delete()
    await user.delete()

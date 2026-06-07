from datetime import timedelta

from beanie import PydanticObjectId
from fastapi import APIRouter, BackgroundTasks, Depends, Query, status

from app.auth.emails import send_token_created_email
from app.auth.models import User
from app.core.config import settings
from app.core.dependencies import get_current_user
from app.core.errors import APIError, ErrorCode
from app.core.ip_hash import hash_ip, make_ip_salt, normalize_ip
from app.core.ratelimit import check_rate_limit
from app.core.scopes import decode, is_valid_mask
from app.core.security import generate_api_token
from app.core.utils import utcnow
from app.tokens.models import ApiToken
from app.tokens.schemas import (
    CreateTokenRequest,
    EditTokenRequest,
    RevokeTokenRequest,
    TokenCreatedResponse,
    TokenPublic,
)
from app.usage.schemas import ActivitySummary
from app.usage.service import aggregate_activity

router = APIRouter(prefix="/tokens", tags=["tokens"])


def _to_public(token: ApiToken) -> TokenPublic:
    return TokenPublic(
        id=str(token.id),
        name=token.name,
        prefix=token.prefix,
        scopes=token.scopes,
        scope_names=decode(token.scopes),
        allowed_ip_count=len(token.allowed_ip_hashes),
        revoked=token.revoked,
        revoked_at=token.revoked_at,
        revoke_reason=token.revoke_reason,
        created_at=token.created_at,
        last_used_at=token.last_used_at,
        rotated_at=token.rotated_at,
        expires_at=token.expires_at,
        request_count=token.request_count,
    )


def _hash_pinned_ips(ips: list[str], salt: str) -> list[str]:
    """Validate, dedupe, and hash the pinned-IP list. Raises 400 on anything
    invalid. An empty list is valid — IP pinning is opt-in. CIDRs are rejected
    here (the underlying ``normalize_ip`` does the check) since hashes can't
    range-match; the error message says so."""
    hashes: list[str] = []
    seen: set[str] = set()
    for raw in ips:
        entry = raw.strip()
        if not entry:
            continue
        try:
            canon = normalize_ip(entry)
        except ValueError as e:
            raise APIError(
                status_code=400, code=ErrorCode.bad_request,
                message=str(e) if "CIDR" in str(e) else f"Invalid IP: {entry}",
            ) from e
        if canon in seen:
            continue
        seen.add(canon)
        hashes.append(hash_ip(salt, canon))
    return hashes


@router.post("", response_model=TokenCreatedResponse, status_code=status.HTTP_201_CREATED)
async def create_token(
    payload: CreateTokenRequest,
    background_tasks: BackgroundTasks,
    user: User = Depends(get_current_user),
) -> TokenCreatedResponse:
    if settings.require_verified_for_tokens and not user.is_verified:
        raise APIError(
            status_code=403,
            code=ErrorCode.email_unverified,
            message="Verify your email address before creating API tokens.",
        )

    # Cap how many tokens a user can mint per day.
    await check_rate_limit(f"tokencreate:{user.id}", settings.token_creation_daily_limit, 86400)

    # Scope bitmask: 0 (all) is fine; otherwise it must use only known bits.
    if not is_valid_mask(payload.scopes):
        raise APIError(
            status_code=400,
            code=ErrorCode.bad_request,
            message="Invalid scope bitmask — it sets bits that aren't real scopes",
        )

    # Generate the per-token salt up-front so the field is always set — even
    # for tokens with no pinned IPs today, a later PATCH that adds IPs will
    # reuse this salt without a separate migration.
    salt = make_ip_salt()
    allowed_ip_hashes = _hash_pinned_ips(payload.allowed_ips, salt)

    full_token, hashed, prefix = generate_api_token()
    expires_at = None
    if payload.expires_in_days is not None:
        expires_at = utcnow() + timedelta(days=payload.expires_in_days)

    assert user.id is not None  # persisted user always has an id
    token = ApiToken(
        user_id=user.id,
        name=payload.name,
        prefix=prefix,
        hashed_token=hashed,
        scopes=payload.scopes,
        ip_salt=salt,
        allowed_ip_hashes=allowed_ip_hashes,
        expires_at=expires_at,
    )
    await token.insert()

    if settings.security_email_notifications:
        background_tasks.add_task(send_token_created_email, user, token.name, token.prefix)

    public = _to_public(token)
    # The secret is returned only here — it is never retrievable again.
    return TokenCreatedResponse(**public.model_dump(), token=full_token)


@router.get("", response_model=list[TokenPublic])
async def list_tokens(user: User = Depends(get_current_user)) -> list[TokenPublic]:
    tokens = await ApiToken.find(ApiToken.user_id == user.id).to_list()
    return [_to_public(t) for t in tokens]


@router.get("/activity", response_model=ActivitySummary)
async def my_activity(
    days: int = Query(default=7, ge=1, le=365),
    user: User = Depends(get_current_user),
) -> ActivitySummary:
    """Aggregate API usage across all of your tokens."""
    return await aggregate_activity({"user_id": user.id}, days)


@router.get("/{token_id}/activity", response_model=ActivitySummary)
async def token_activity(
    token_id: PydanticObjectId,
    days: int = Query(default=7, ge=1, le=365),
    user: User = Depends(get_current_user),
) -> ActivitySummary:
    """Aggregate API usage for one of your tokens."""
    token = await _owned_token(token_id, user)
    return await aggregate_activity({"token_id": token.id}, days)


async def _owned_token(token_id: PydanticObjectId, user: User) -> ApiToken:
    token = await ApiToken.get(token_id)
    if token is None or token.user_id != user.id:
        raise APIError(status_code=404, code=ErrorCode.not_found, message="Token not found")
    return token


@router.patch("/{token_id}", response_model=TokenPublic)
async def edit_token(
    token_id: PydanticObjectId,
    payload: EditTokenRequest,
    user: User = Depends(get_current_user),
) -> TokenPublic:
    """Edit a token's name and/or allowed IPs. Secret and scopes are immutable."""
    token = await _owned_token(token_id, user)
    if token.revoked:
        raise APIError(status_code=400, code=ErrorCode.bad_request, message="Token is revoked")

    if payload.name is not None:
        token.name = payload.name
    if payload.allowed_ips is not None:
        # Backfill salt for legacy tokens (created before the hash migration
        # but never patched since). Brand-new tokens have one minted on insert.
        if not token.ip_salt:
            token.ip_salt = make_ip_salt()
        token.allowed_ip_hashes = _hash_pinned_ips(payload.allowed_ips, token.ip_salt)
    await token.save()
    return _to_public(token)


@router.post("/{token_id}/rotate", response_model=TokenCreatedResponse)
async def rotate_token(
    token_id: PydanticObjectId,
    background_tasks: BackgroundTasks,
    user: User = Depends(get_current_user),
) -> TokenCreatedResponse:
    """Issue a fresh secret for a token, keeping its name, scopes, IPs and expiry.

    The old secret stops working immediately. The new secret is shown only once,
    exactly like creation. Useful when a key may be compromised but you don't
    want to re-grant scopes or update allowlists.
    """
    token = await _owned_token(token_id, user)
    if token.revoked:
        raise APIError(status_code=400, code=ErrorCode.bad_request, message="Token is revoked")

    full_token, hashed, prefix = generate_api_token()
    token.hashed_token = hashed
    token.prefix = prefix
    token.rotated_at = utcnow()
    token.last_used_at = None
    token.expiry_warned = False
    await token.save()

    if settings.security_email_notifications:
        background_tasks.add_task(send_token_created_email, user, token.name, token.prefix)

    public = _to_public(token)
    return TokenCreatedResponse(**public.model_dump(), token=full_token)


@router.post("/{token_id}/revoke", response_model=TokenPublic)
async def revoke_token(
    token_id: PydanticObjectId,
    payload: RevokeTokenRequest,
    user: User = Depends(get_current_user),
) -> TokenPublic:
    """Revoke a token. A reason is required."""
    token = await _owned_token(token_id, user)
    if not token.revoked:
        token.revoked = True
        token.revoked_at = utcnow()
        token.revoke_reason = payload.reason
        await token.save()
    return _to_public(token)

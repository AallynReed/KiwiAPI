"""The Discord permission algorithm used for bot preflight + manage checks."""
from app.bot.discord_rest import (
    _ALL,
    PERM_ADMINISTRATOR,
    PERM_MANAGE_GUILD,
    PERM_SEND_MESSAGES,
    PERM_VIEW_CHANNEL,
    effective_channel_permissions,
    guild_permissions,
)

GUILD_ID = 100
EVERYONE = {"id": str(GUILD_ID), "permissions": str(PERM_VIEW_CHANNEL | PERM_SEND_MESSAGES)}


def _guild(roles, owner=999):
    return {"id": str(GUILD_ID), "owner_id": str(owner), "roles": roles}


def _member(uid, roles):
    return {"user": {"id": str(uid)}, "roles": [str(r) for r in roles]}


def test_owner_has_everything():
    assert guild_permissions(_guild([EVERYONE], owner=42), _member(42, [])) == _ALL


def test_administrator_role_grants_all():
    admin = {"id": "5", "permissions": str(PERM_ADMINISTRATOR)}
    assert guild_permissions(_guild([EVERYONE, admin]), _member(7, [5])) == _ALL


def test_manage_guild_is_detected():
    mod = {"id": "5", "permissions": str(PERM_MANAGE_GUILD)}
    perms = guild_permissions(_guild([EVERYONE, mod]), _member(7, [5]))
    assert perms & PERM_MANAGE_GUILD


def test_plain_member_has_no_manage():
    perms = guild_permissions(_guild([EVERYONE]), _member(7, []))
    assert not (perms & (PERM_ADMINISTRATOR | PERM_MANAGE_GUILD))


def test_channel_everyone_deny_removes_send():
    channel = {"id": "200", "permission_overwrites": [
        {"id": str(GUILD_ID), "type": 0, "allow": "0", "deny": str(PERM_SEND_MESSAGES)},
    ]}
    perms = effective_channel_permissions(_guild([EVERYONE]), _member(7, []), channel)
    assert perms & PERM_VIEW_CHANNEL
    assert not (perms & PERM_SEND_MESSAGES)        # denied at the channel level


def test_role_overwrite_reallows_over_everyone_deny():
    role = {"id": "5", "permissions": "0"}
    channel = {"id": "200", "permission_overwrites": [
        {"id": str(GUILD_ID), "type": 0, "allow": "0", "deny": str(PERM_SEND_MESSAGES)},
        {"id": "5", "type": 0, "allow": str(PERM_SEND_MESSAGES), "deny": "0"},
    ]}
    perms = effective_channel_permissions(_guild([EVERYONE, role]), _member(7, [5]), channel)
    assert perms & PERM_SEND_MESSAGES              # role overwrite wins over @everyone deny


def test_member_overwrite_wins_last():
    channel = {"id": "200", "permission_overwrites": [
        {"id": "7", "type": 1, "allow": "0", "deny": str(PERM_VIEW_CHANNEL)},
    ]}
    perms = effective_channel_permissions(_guild([EVERYONE]), _member(7, []), channel)
    assert not (perms & PERM_VIEW_CHANNEL)         # member-specific deny is final


# ── role-delegated capability resolution (Phase 2) ──────────────────────────

def test_capability_admin_has_everything():
    from app.bot.router import _has_capability
    assert _has_capability({"is_admin": True, "role_ids": set()}, {}, "manage_announcements")


def test_capability_granted_by_held_role():
    from app.bot.router import _has_capability
    ctx = {"is_admin": False, "role_ids": {10, 20}}
    assert _has_capability(ctx, {"manage_announcements": [20]}, "manage_announcements")


def test_capability_denied_without_matching_role():
    from app.bot.router import _has_capability
    ctx = {"is_admin": False, "role_ids": {10, 20}}
    assert not _has_capability(ctx, {"manage_announcements": [99]}, "manage_announcements")


def test_capability_deleted_or_unheld_role_does_not_grant():
    from app.bot.router import _has_capability
    # mapped role 99 isn't in the user's live role_ids (deleted, or never held)
    assert not _has_capability({"is_admin": False, "role_ids": {10}},
                               {"manage_announcements": [99]}, "manage_announcements")
    # ...but another mapped role they DO hold still grants (fallback)
    assert _has_capability({"is_admin": False, "role_ids": {10, 99}},
                           {"manage_announcements": [99]}, "manage_announcements")


def test_ping_capability_is_independent_of_announcements():
    from app.bot.router import _has_capability
    ctx = {"is_admin": False, "role_ids": {10}}
    perms = {"manage_announcements": [10], "manage_ping_roles": [20]}
    # holds the announcements role but not the ping role -> can configure, can't ping
    assert _has_capability(ctx, perms, "manage_announcements")
    assert not _has_capability(ctx, perms, "manage_ping_roles")


def test_ping_capability_granted_by_held_role():
    from app.bot.router import _has_capability
    ctx = {"is_admin": False, "role_ids": {20}}
    assert _has_capability(ctx, {"manage_ping_roles": [20]}, "manage_ping_roles")


def test_admin_has_every_capability():
    from app.bot.router import CAPABILITIES, _has_capability
    ctx = {"is_admin": True, "role_ids": set()}
    assert all(_has_capability(ctx, {}, cap) for cap in CAPABILITIES)

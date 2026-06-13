"""GuildConfig reconciliation + legacy migration (pure, no DB).

``reconcile`` keeps the stored config in lock-step with the guild's live Discord
state; ``migrate_legacy`` folds the old single-challenge fields into the generic
``announcements`` map. Both are pure mutations, so we build configs with
``model_construct`` (Beanie's ``__init__`` wants a live collection otherwise).
"""
from app.bot.models import AnnouncementSetting, Club, GuildConfig, LiveBoard
from app.bot.reconcile import reconcile, reconcile_club


def _cfg(**kw) -> GuildConfig:
    kw.setdefault("announcements", {})
    kw.setdefault("config_perms", {})
    kw.setdefault("live_board", LiveBoard())
    return GuildConfig.model_construct(**kw)


# ── reconcile: channels ──────────────────────────────────────────────────────

def test_deleted_channel_disables_and_flags():
    cfg = _cfg(announcements={
        "chaos_chest": AnnouncementSetting(enabled=True, channel_id=900),
    })
    assert reconcile(cfg, live_channel_ids={1, 2}, live_role_ids=set()) is True
    s = cfg.announcements["chaos_chest"]
    assert s.enabled is False          # disabled (can't post to a dead channel)
    assert s.channel_missing is True   # ...but kept + flagged loudly for the user
    assert s.channel_id == 900         # id retained so the warning is meaningful


def test_live_channel_is_left_alone():
    cfg = _cfg(announcements={
        "stampy": AnnouncementSetting(enabled=True, channel_id=901),
    })
    assert reconcile(cfg, live_channel_ids={901}, live_role_ids=set()) is False
    s = cfg.announcements["stampy"]
    assert s.enabled is True and s.channel_missing is False


def test_channel_missing_clears_when_repointed_to_live_channel():
    cfg = _cfg(announcements={
        "stampy": AnnouncementSetting(enabled=False, channel_id=901, channel_missing=True),
    })
    assert reconcile(cfg, live_channel_ids={901}, live_role_ids=set()) is True
    assert cfg.announcements["stampy"].channel_missing is False


# ── reconcile: roles ─────────────────────────────────────────────────────────

def test_deleted_ping_roles_are_pruned():
    cfg = _cfg(announcements={
        "chaos_chest": AnnouncementSetting(enabled=True, channel_id=1, ping_role_ids=[50, 60]),
    })
    assert reconcile(cfg, live_channel_ids={1}, live_role_ids={60}) is True
    assert cfg.announcements["chaos_chest"].ping_role_ids == [60]   # 50 deleted -> dropped


def test_deleted_board_channel_disables_flags_and_drops_message():
    cfg = _cfg(live_board=LiveBoard(enabled=True, channel_id=900, message_id=123))
    assert reconcile(cfg, live_channel_ids={1}, live_role_ids=set()) is True
    b = cfg.live_board
    assert b.enabled is False and b.channel_missing is True
    assert b.message_id is None        # the old message is gone with the channel


def test_live_board_on_a_live_channel_is_untouched():
    cfg = _cfg(live_board=LiveBoard(enabled=True, channel_id=900, message_id=123))
    assert reconcile(cfg, live_channel_ids={900}, live_role_ids=set()) is False
    assert cfg.live_board.enabled is True and cfg.live_board.message_id == 123


def test_deleted_role_pruned_from_capability_grants():
    cfg = _cfg(config_perms={"manage_announcements": [50, 60], "manage_ping_roles": [51]})
    assert reconcile(cfg, live_channel_ids=set(), live_role_ids={60}) is True
    # 50 pruned (kept 60); manage_ping_roles emptied -> dropped entirely
    assert cfg.config_perms == {"manage_announcements": [60]}


def test_noop_when_everything_is_live():
    cfg = _cfg(
        announcements={"stampy": AnnouncementSetting(enabled=True, channel_id=1, ping_role_ids=[50])},
        config_perms={"manage_announcements": [50]},
    )
    assert reconcile(cfg, live_channel_ids={1}, live_role_ids={50}) is False


# ── legacy migration ─────────────────────────────────────────────────────────

def test_migrate_legacy_folds_challenge_fields_once():
    cfg = _cfg(hourly_challenge_enabled=True, announce_channel_id=5,
               last_announced_challenge_anchor=100)
    assert cfg.migrate_legacy() is True
    s = cfg.announcements["hourly_challenge"]
    assert s.enabled is True and s.channel_id == 5 and s.last_anchor == "100"
    # legacy fields cleared so they can't double-fire
    assert cfg.hourly_challenge_enabled is False
    assert cfg.announce_channel_id is None
    assert cfg.last_announced_challenge_anchor is None
    # idempotent
    assert cfg.migrate_legacy() is False


def test_migrate_legacy_noop_on_fresh_config():
    cfg = _cfg()
    assert cfg.migrate_legacy() is False
    assert cfg.announcements == {}


# ── clubs: drop rank->role links whose role was deleted ──────────────────────

def test_reconcile_club_drops_deleted_role_links():
    club = Club.model_construct(guild_id=1, name="Vanguard",
                                role_links={"president": 50, "officer": 60, "member": 70})
    assert reconcile_club(club, live_role_ids={60, 70}) is True   # 50 deleted
    assert club.role_links == {"officer": 60, "member": 70}
    assert reconcile_club(club, live_role_ids={60, 70}) is False  # idempotent


def test_reconcile_club_noop_when_all_links_live():
    club = Club.model_construct(guild_id=1, name="Vanguard", role_links={"captain": 5})
    assert reconcile_club(club, live_role_ids={5}) is False

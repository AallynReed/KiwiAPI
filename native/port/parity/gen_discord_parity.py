"""Parity specs for the Discord area, from the repo root:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_discord_parity.py
Writes discord.json (production GET/HEAD), discord.writes.json + discord.seed.js
+ discord.state.json + discord.env (twin sandbox). Interactions are signed with
a fixed sandbox Ed25519 key (its public half goes in discord.env); no request
reaches Discord: the sandbox has no bot token and no way out.
"""
import json
from pathlib import Path

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

HERE = Path(__file__).resolve().parent
KEY = Ed25519PrivateKey.from_private_bytes(bytes(range(1, 33)))
PUB = KEY.public_key().public_bytes_raw().hex()
TS = "1792000000"

LINKED_JWT = ("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiI2NWYwMDAwMDAwMDAwMDAwMDAwMDAwYTEiLCJ2ZXIiOjAsImlhdCI6MTc5MjAw"
              "MDAwMCwiZXhwIjoxODkzNDU2MDAwLCJ0eXBlIjoiYWNjZXNzIiwia2luZCI6InNpdGUifQ.3WdiSLUG7cmS6wO0ek_miBiZyGv0CkMVVCUM_u-0kTg")
UNLINKED_JWT = ("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiI2NWYwMDAwMDAwMDAwMDAwMDAwMDAwYTIiLCJ2ZXIiOjAsImlhdCI6MTc5Mj"
                "AwMDAwMCwiZXhwIjoxODkzNDU2MDAwLCJ0eXBlIjoiYWNjZXNzIiwia2luZCI6InNpdGUifQ.IelE7UqjZVPsJTteFjX_6P6m0Ers_V0szPvgxLK1ZUA")
GID = "4242424242424242"
P = "/v1/site-auth/discord"


def signed(name, body: str, ts=TS, sig=None):
    s = sig if sig is not None else KEY.sign(ts.encode() + body.encode()).hex()
    return {"name": name, "method": "POST", "path": "/discord/interactions", "body": body,
            "headers": {"Content-Type": "application/json", "X-Signature-Ed25519": s, "X-Signature-Timestamp": ts}}


def cmd(name, guild=None, locale=None, options=None):
    d = {"type": 2, "data": {"name": name}}
    if options:
        d["data"]["options"] = [{"name": k, "value": v} for k, v in options.items()]
    if guild:
        d["guild_id"] = guild
    if locale:
        d["locale"] = locale
    return json.dumps(d, separators=(",", ":"))


def production():
    a = {"Authorization": "Bearer ${KIWI_PARITY_JWT}"}
    t = {"Authorization": "Bearer ${KIWI_PARITY_TOKEN}"}
    return [
        {"name": "meta_noauth", "path": f"{P}/announcement-meta"},
        {"name": "meta_head_noauth", "method": "HEAD", "path": f"{P}/announcement-meta"},
        {"name": "meta_portal_jwt", "path": f"{P}/announcement-meta", "headers": a},
        {"name": "meta_api_token", "path": f"{P}/announcement-meta", "headers": t},
        {"name": "meta_cookie_foreign_origin", "path": f"{P}/announcement-meta",
         "headers": {"Cookie": "kiwi_site_access=abc", "Origin": "https://evil.example"}},
        {"name": "guilds_noauth", "path": f"{P}/guilds"},
        {"name": "guilds_bad_bearer", "path": f"{P}/guilds", "headers": {"Authorization": "Bearer nope"}},
        {"name": "guild_noauth", "path": f"{P}/guilds/{GID}"},
        {"name": "guild_bad_id_noauth", "path": f"{P}/guilds/abc"},
        {"name": "guild_huge_id_noauth", "path": f"{P}/guilds/99999999999999999999"},
        {"name": "clubs_noauth", "path": f"{P}/guilds/{GID}/clubs"},
        {"name": "roster_noauth", "path": f"{P}/guilds/{GID}/clubs/65e000000000000000000001/roster"},
        {"name": "announcements_get_405", "path": f"{P}/guilds/{GID}/announcements"},
        {"name": "template_get_405", "path": f"{P}/guilds/{GID}/announcements/stampy/template"},
        {"name": "language_get_405", "path": f"{P}/guilds/{GID}/language"},
        {"name": "board_head_405", "method": "HEAD", "path": f"{P}/guilds/{GID}/live-board"},
        {"name": "market_get_405", "path": f"{P}/guilds/{GID}/market-watch"},
        {"name": "club_get_405", "path": f"{P}/guilds/{GID}/clubs/65e000000000000000000001"},
        {"name": "roster_action_get_405", "path": f"{P}/guilds/{GID}/clubs/x/roster/1/promote"},
        {"name": "permissions_get_405", "path": f"{P}/guilds/{GID}/permissions"},
        {"name": "guilds_trailing_slash", "path": f"{P}/guilds/"},
        {"name": "deep_404", "path": f"{P}/guilds/{GID}/nope/deeper"},
        {"name": "options_preflight", "method": "OPTIONS", "path": f"{P}/guilds",
         "headers": {"Origin": "https://trove.aallyn.net", "Access-Control-Request-Method": "GET"},
         "compare_headers": ["access-control-allow-origin", "access-control-allow-methods",
                             "access-control-allow-credentials"]},
        {"name": "interactions_get_405", "path": "/discord/interactions"},
        {"name": "interactions_head_405", "method": "HEAD", "path": "/discord/interactions"},
    ]


def writes():
    L = {"Authorization": f"Bearer {LINKED_JWT}"}
    U = {"Authorization": f"Bearer {UNLINKED_JWT}"}
    out = [
        {"name": "ix_no_signature", "method": "POST", "path": "/discord/interactions", "body": '{"type":1}',
         "headers": {"Content-Type": "application/json"}},
        signed("ix_bad_signature", '{"type":1}', sig="00" * 64),
        signed("ix_wrong_timestamp", '{"type":1}', sig=KEY.sign(b"1" + b'{"type":1}').hex()),
        signed("ix_ping", '{"type":1}'),
        signed("ix_ping_spaced", '{"type": 1, "x": "é"}'),
        signed("ix_empty_body", ""),
        signed("ix_bad_json", '{"type":'),
        signed("ix_list_body", "[1,2]"),
        signed("ix_unsupported", '{"type":3,"data":{"name":"status"}}'),
        signed("ix_unknown", cmd("nope")),
        signed("ix_unknown_fr_guild", cmd("nope", guild=GID)),
        signed("ix_web", cmd("web")),
        signed("ix_web_locale", cmd("web", locale="pt-BR")),
        signed("ix_web_guild_fr", cmd("web", guild=GID)),
        signed("ix_status", cmd("status")),
        signed("ix_status_fr", cmd("status", guild=GID)),
        signed("ix_hourly", cmd("hourly_challenge")),
        signed("ix_bonuses", cmd("bonuses", guild=GID)),
        signed("ix_longshade", cmd("longshade")),
        signed("ix_wild_mana", cmd("wild_mana", locale="ja")),
        signed("ix_corruxion", cmd("corruxion")),
        signed("ix_fluxion", cmd("fluxion", guild=GID)),
        signed("ix_stampy", cmd("stampy")),
        signed("ix_giveaways", cmd("giveaways")),
        signed("ix_news", cmd("trove_news")),
        signed("ix_uptime", cmd("trove_uptime")),
        signed("ix_download", cmd("download")),
        signed("ix_change_log", cmd("change_log", guild=GID)),
        signed("ix_chaos", cmd("chaos")),
        signed("ix_price_missing", cmd("price", options={"item": "  Nothing Here "})),
        signed("ix_price_blank", cmd("price", options={"item": "   "})),
        signed("ix_gem", cmd("gem", options={"tier": 4, "type": 2, "level": 20, "stat1": 1, "value1": 512.4,
                                             "stat2": 4, "value2": 25, "stat3": 6, "value3": 30.5})),
        signed("ix_gem_bad_tier", cmd("gem", guild=GID, options={"tier": 9, "type": 2, "level": 20})),
        signed("ix_gem_dupe", cmd("gem", options={"tier": 1, "type": 1, "level": 1, "stat1": 1, "value1": 1,
                                                  "stat2": 1, "value2": 2, "stat3": 6, "value3": 3})),
        {"name": "meta", "path": f"{P}/announcement-meta", "headers": L},
        {"name": "guilds_linked_no_token", "path": f"{P}/guilds", "headers": L},
        {"name": "guilds_unlinked", "path": f"{P}/guilds", "headers": U},
        {"name": "detail_unlinked", "path": f"{P}/guilds/{GID}", "headers": U},
        {"name": "detail_no_bot_token", "path": f"{P}/guilds/{GID}", "headers": L},
        {"name": "detail_bad_id", "path": f"{P}/guilds/x1", "headers": L},
        {"name": "ann_422", "method": "PUT", "path": f"{P}/guilds/{GID}/announcements", "headers": L,
         "body": {"announcements": {"a": {"enabled": "maybe", "channel_id": 5}}}},
        {"name": "ann_422_path_body", "method": "PUT", "path": f"{P}/guilds/x/announcements", "headers": L,
         "body": {}},
        {"name": "ann_json_invalid", "method": "PUT", "path": f"{P}/guilds/{GID}/announcements",
         "headers": dict(L, **{"Content-Type": "application/json"}), "body": '{"announcements"'},
        {"name": "ann_no_bot_token", "method": "PUT", "path": f"{P}/guilds/{GID}/announcements", "headers": L,
         "body": {"announcements": {}}},
        {"name": "tmpl_unknown", "method": "PUT", "path": f"{P}/guilds/{GID}/announcements/luxion/template",
         "headers": L, "body": {}},
        {"name": "tmpl_422", "method": "PUT", "path": f"{P}/guilds/{GID}/announcements/stampy/template",
         "headers": L, "body": {"template": {"fields": [{"name": 1}]}}},
        {"name": "lang_422", "method": "PUT", "path": f"{P}/guilds/{GID}/language", "headers": L,
         "body": {"language": 3}},
        {"name": "lang_unlinked", "method": "PUT", "path": f"{P}/guilds/{GID}/language", "headers": U,
         "body": {"language": "fr"}},
        {"name": "board_no_bot_token", "method": "PUT", "path": f"{P}/guilds/{GID}/live-board", "headers": L,
         "body": {"enabled": True, "channel_id": "1"}},
        {"name": "mw_422", "method": "PUT", "path": f"{P}/guilds/{GID}/market-watch", "headers": L,
         "body": {"items": [{"max_price_each": "x"}]}},
        {"name": "clubs_no_bot_token", "path": f"{P}/guilds/{GID}/clubs", "headers": L},
        {"name": "club_create_unlinked", "method": "POST", "path": f"{P}/guilds/{GID}/clubs", "headers": U},
        {"name": "club_update_422", "method": "PUT", "path": f"{P}/guilds/{GID}/clubs/x", "headers": L,
         "body": {"role_links": {"a": 1}}},
        {"name": "club_delete_no_bot_token", "method": "DELETE", "path": f"{P}/guilds/{GID}/clubs/x", "headers": L},
        {"name": "roster_bad_action", "method": "POST", "path": f"{P}/guilds/{GID}/clubs/x/roster/5/kick",
         "headers": L},
        {"name": "roster_bad_member", "method": "POST", "path": f"{P}/guilds/{GID}/clubs/x/roster/y/promote",
         "headers": L},
        {"name": "perms_422", "method": "PUT", "path": f"{P}/guilds/{GID}/permissions", "headers": L,
         "body": {"permissions": {"a": "b"}}},
        {"name": "perms_no_bot_token", "method": "PUT", "path": f"{P}/guilds/{GID}/permissions", "headers": L,
         "body": {"permissions": {}}},
    ]
    return out


SEED = """// Two site users (one linked to Discord), a French guild config and a club,
// shaped as Beanie writes them. Specs sign in with fixed site JWTs minted for
// these ids against the sandbox SECRET_KEY.
const T0 = ISODate("2026-09-01T12:00:00.250Z");
const LINKED = ObjectId("65f0000000000000000000a1"), UNLINKED = ObjectId("65f0000000000000000000a2");
const user = (_id, username, discord_id) => ({
  _id, username, discord_handle: username, notify_email: null, display_name: null, is_active: true,
  is_verified: true, is_deleted: false, discord_id, discord_avatar: null, token_version: NumberInt(0),
  claimed_trove_name: null, claimed_trove_display: null, claimed_at: null, claim_verified: false,
  claim_baseline: {}, created_at: T0, updated_at: T0,
});
db.site_users.insertMany([user(LINKED, "paritylinked", NumberLong("312345678901234567")),
                          user(UNLINKED, "parityunlinked", null)]);
db.guild_configs.insertOne({
  _id: ObjectId("65d000000000000000000001"), guild_id: NumberLong("4242424242424242"), announcements: {},
  live_board: {enabled: false, channel_id: null, message_id: null, channel_missing: false},
  market_watch: {enabled: false, channel_id: null, ping_role_ids: [], channel_missing: false, items: []},
  language: "fr", config_perms: {}, hourly_challenge_enabled: false, announce_channel_id: null,
  last_announced_challenge_anchor: null, updated_by: null, created_at: T0, updated_at: T0,
});
db.clubs.insertOne({
  _id: ObjectId("65e000000000000000000001"), guild_id: NumberLong("4242424242424242"), name: "Alpha",
  public: false, description: null, banner_url: null, avatar_url: null, discord_url: null, website_url: null,
  role_links: {}, updated_by: null, created_at: T0, updated_at: T0,
});
"""

STATE = {
    "mongo": [
        {"collection": "guild_configs", "sort": ["guild_id"], "mask": ["updated_at"]},
        {"collection": "clubs", "sort": ["name"]},
        {"collection": "bot_command_usage", "sort": ["_id"], "mask": ["last_used_at"]},
        {"collection": "site_users", "sort": ["username"]},
    ]
}


def dump(path: Path, specs):
    lines = ",\n ".join(json.dumps(s, ensure_ascii=False) for s in specs)
    path.write_text("[\n " + lines + "\n]\n", encoding="utf-8")


def main():
    dump(HERE / "discord.json", production())
    dump(HERE / "discord.writes.json", writes())
    (HERE / "discord.seed.js").write_text(SEED, encoding="utf-8")
    (HERE / "discord.state.json").write_text(json.dumps(STATE, indent=1) + "\n", encoding="utf-8")
    (HERE / "discord.env").write_text(
        "# Sandbox interactions key (its private half signs discord.writes.json); no bot token.\n"
        f"DISCORD_PUBLIC_KEY={PUB}\n", encoding="utf-8")
    print("wrote discord.json, discord.writes.json, discord.seed.js, discord.state.json, discord.env")


if __name__ == "__main__":
    main()

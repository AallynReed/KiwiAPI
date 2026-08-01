"""Per-event variable contexts + default embed templates for webhooks.

Each deliverable event type exposes:
- ``context(data)``        - the live variables for the shared renderer,
- ``default_template()``   - the out-of-the-box embed (English; users override it),
- ``sample_context()``     - representative values for the editor palette + preview.

Rendering goes through ``app.embed_templates`` so a user's custom template and the
default share one code path. English-only (a webhook posts to an arbitrary Discord
with no guild-language context).
"""

from __future__ import annotations

from app.core.config import settings
from app.embed_templates import EmbedField, EmbedTemplate, render_template

SITE = "https://trove.aallyn.net"          # page links (on the website host)
# Off-site-embedded API renders (announce PNG) - app_url today, api_url at
# cutover once the pages move off the api host. See config.asset_url.
_ASSET = settings.asset_url

_COLOR_CHALLENGE = "#F2A33C"
_COLOR_MOD_RELEASE = "#46D39A"
_COLOR_GAME_UPDATE = "#5EC6FF"


def _num(v) -> str:
    try:
        return f"{int(v):,}"
    except (TypeError, ValueError):
        return "0"


def _trunc(s, n: int) -> str:
    s = (str(s or "")).strip()
    return s if len(s) <= n else s[: n - 1].rstrip() + "…"


def _challenge_context(d: dict) -> dict:
    name = d.get("name") or "Challenge"
    ctype = d.get("type") or ""
    return {
        "name": name,
        "type": ctype,
        "type_label": ctype.replace("_", " ").title() or "Challenge",
        "starts_at": d.get("starts_at"),
        "ends_at": d.get("ends_at"),
        "status": "live" if d.get("active") else "upcoming",
        "is_friday": bool(d.get("is_friday_window")),
    }


def _challenge_default() -> EmbedTemplate:
    return EmbedTemplate(
        title="⚔️ New challenge — {name}",
        url=f"{SITE}/server-time",
        description="A new hourly challenge is live in Trove.",
        color=_COLOR_CHALLENGE,
        fields=[
            EmbedField(name="Type", value="{type_label}", inline=True),
            EmbedField(name="Ends", value="{ends_at:R}", inline=True),
        ],
        footer="trove.aallyn.net",
        show_image=False,
    )


def _challenge_sample() -> dict:
    return _challenge_context({
        "name": "Racing Challenge", "type": "racing",
        "starts_at": 1718200000, "ends_at": 1718201200,
        "active": True, "is_friday_window": False,
    })


def _bytes(n) -> str:
    """A byte count as KB/MB (blank when unknown, so the field just reads empty)."""
    try:
        b = float(n)
    except (TypeError, ValueError):
        return ""
    if b < 1024:
        return f"{int(b)} B"
    if b < 1024 * 1024:
        return f"{b / 1024:.0f} KB"
    return f"{b / (1024 * 1024):.2f} MB"


def _signed_bytes(n) -> str:
    """A size *delta* against the previous build, e.g. ``+40 KB`` / ``-1.20 MB``."""
    try:
        b = int(n)
    except (TypeError, ValueError):
        return ""
    if b == 0:
        return "no change"
    return f"{'+' if b > 0 else '-'}{_bytes(abs(b))}"


def _mod_release_context(d: dict) -> dict:
    project = d.get("project") or {}
    release = d.get("release") or {}
    previous = d.get("previous") or {}
    change = d.get("change") or {}
    fmt = str(release.get("format") or "")
    prev_tag = previous.get("tag") or ""
    tag = release.get("tag") or ""
    return {
        "project_title": project.get("title") or project.get("slug") or "Mod",
        "project_slug": project.get("slug") or "",
        "handle": project.get("handle") or "",
        "owner": project.get("owner") or "unknown",
        # The creator credit: `author` is the named third party on an uploaded /
        # stray mod, and the owner on an ordinary authored one.
        "author": project.get("author") or project.get("owner") or "unknown",
        "summary": _trunc(project.get("summary") or "", 200),
        "tags": ", ".join(t for t in (project.get("tags") or []) if t),
        "downloads": _num(project.get("download_count")),
        "stars": _num(project.get("star_count")),
        "tag": tag,
        "release_title": release.get("title") or tag or "New release",
        "branch": release.get("branch") or "",
        "format": fmt,
        "format_upper": fmt.upper(),
        "filename": release.get("filename") or "",
        # The .tmod header's own version string (empty for a .zip release).
        "mod_version": release.get("mod_version") or "",
        "sha256": release.get("sha256") or "",
        "sha_short": (release.get("sha256") or "")[:12],
        "size": release.get("size"),
        "size_label": _bytes(release.get("size")),
        "changelog": _trunc(release.get("changelog") or "", 1000),
        # --- what changed since the previous build on this variant --------
        "previous_tag": prev_tag,
        "version_change": f"{prev_tag} → {tag}" if prev_tag else tag,
        "size_delta": _signed_bytes(change.get("size_delta")),
        "release_count": _num(change.get("release_count")),
        "update_kind": "First release" if change.get("is_first_release") else "Update",
        "download_url": d.get("download_url") or "",
        "page_url": d.get("page_url") or SITE,
        # Drives the embed image (the build's preview, else the mod banner).
        "image_url": d.get("image_url") or "",
    }


def _mod_release_default() -> EmbedTemplate:
    return EmbedTemplate(
        title="📦 {project_title} — {release_title}",
        url="{page_url}",
        description="{changelog}",
        color=_COLOR_MOD_RELEASE,
        fields=[
            # "v1.2.0 → v1.3.0" on an update; just the tag on a first release.
            EmbedField(name="Version", value="{version_change}", inline=True),
            EmbedField(name="Variant", value="{branch}", inline=True),
            EmbedField(name="Format", value="{format_upper}", inline=True),
            EmbedField(name="Size", value="{size_label}", inline=True),
            EmbedField(name="Download", value="[Get it]({download_url})", inline=True),
        ],
        footer="Mods Hub · by {author}",
        show_image=True,
    )


def _mod_release_sample() -> dict:
    return _mod_release_context({
        "project": {"title": "Neon HUD", "slug": "neon-hud", "handle": "aallyn",
                    "owner": "Aallyn", "author": "Aallyn",
                    "summary": "A cleaner, higher-contrast HUD.",
                    "tags": ["ui", "hud"], "download_count": 4820, "star_count": 96},
        "release": {"tag": "v1.3.0", "title": "Stable", "branch": "main",
                    "format": "tmod", "changelog": "Cleaner icons + bug fixes.",
                    "filename": "Neon HUD.tmod", "size": 1_482_240,
                    "mod_version": "1.3.0",
                    "sha256": "9f2c1ab4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f708192a3b4c5d6e7f80"},
        "previous": {"tag": "v1.2.0"},
        "change": {"size_delta": 40_960, "release_count": 7, "is_first_release": False},
        # A real mod's event carries its own preview/banner; the editor preview
        # just needs a stand-in that resolves.
        "image_url": f"{_ASSET}/static/assets/mod_manager.png",
        "download_url": f"{SITE}/mods/aallyn/neon-hud", "page_url": f"{SITE}/mods/aallyn/neon-hud",
    })


def _game_update_context(d: dict) -> dict:
    v = d.get("version") or {}
    ordinal = v.get("ordinal")
    return {
        "version_tag": v.get("version_tag") or "new build",
        "ordinal": ordinal,
        "branch": v.get("branch") or "",
        "files_added": _num(v.get("files_added")),
        "files_modified": _num(v.get("files_modified")),
        "files_removed": _num(v.get("files_removed")),
        # convenience: the announcement banner image for this build
        "image_url": (f"{_ASSET}/announce.png?kind=game_update&v={ordinal}"
                      if ordinal is not None else None),
    }


def _game_update_default() -> EmbedTemplate:
    return EmbedTemplate(
        title="🧩 New Trove update — {version_tag}",
        url=f"{SITE}/updates",
        description="A new build is live on the US servers. See what changed:",
        color=_COLOR_GAME_UPDATE,
        fields=[
            EmbedField(name="Added", value="{files_added}", inline=True),
            EmbedField(name="Modified", value="{files_modified}", inline=True),
            EmbedField(name="Removed", value="{files_removed}", inline=True),
        ],
        footer="Browse the changed files at trove.aallyn.net/updates",
        show_image=True,
    )


def _game_update_sample() -> dict:
    return _game_update_context({"version": {
        "version_tag": "2024.06.01", "ordinal": 142, "branch": "live-us",
        "files_added": 12, "files_modified": 8, "files_removed": 1,
    }})


_TYPES = {
    "challenge": (_challenge_context, _challenge_default, _challenge_sample),
    "mod_release": (_mod_release_context, _mod_release_default, _mod_release_sample),
    "game_update": (_game_update_context, _game_update_default, _game_update_sample),
}


def default_template(event_type: str) -> EmbedTemplate | None:
    spec = _TYPES.get(event_type)
    return spec[1]() if spec else None


def sample_context(event_type: str) -> dict:
    spec = _TYPES.get(event_type)
    return spec[2]() if spec else {}


def variables(event_type: str) -> list[str]:
    """The variable names available to a template for this event (for the editor)."""
    return [k for k in sample_context(event_type) if k != "image_url"]


def _render_ctx(event_type: str, ctx: dict, template: EmbedTemplate | None) -> dict | None:
    spec = _TYPES.get(event_type)
    if spec is None:
        return None
    _, default_fn, _ = spec
    tmpl = template if (template and template.enabled) else default_fn()
    embed, content = render_template(tmpl, ctx, default_image_url=ctx.get("image_url"))
    body: dict = {"embeds": [embed]}
    if content:
        body["content"] = content
    return body


def render(event_type: str, data: dict, template: EmbedTemplate | None = None) -> dict | None:
    """Discord webhook body for one event, using ``template`` if provided & enabled,
    else the type's default."""
    spec = _TYPES.get(event_type)
    if spec is None:
        return None
    return _render_ctx(event_type, spec[0](data or {}), template)


def render_sample(event_type: str, template: EmbedTemplate | None = None) -> dict | None:
    """Render the embed against this event's sample context (for the test/preview)."""
    return _render_ctx(event_type, sample_context(event_type), template)


def test_body() -> dict:
    return {
        "embeds": [{
            "title": "✅ Kiwi webhook connected",
            "color": 0x46D39A,
            "description": "This Discord webhook is wired up. You'll get a message "
                           "here when your subscribed Trove events fire.",
            "footer": {"text": "trove.aallyn.net"},
        }]
    }

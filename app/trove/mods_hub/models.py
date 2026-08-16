"""Mongo models for the Mods Hub.

Branches, commits and file trees are NOT here - they live in the per-project git
repo (``gitstore.py``), which is the source of truth for file content + history
(so a `git push` and a web "Commit files" share one history). Mongo holds the
project metadata, releases (compiled ``.tmod`` artifacts in the CAS), images,
reports and git access tokens.
"""

from datetime import datetime
from typing import Literal

from beanie import Document, PydanticObjectId
from pydantic import BaseModel, Field
from pymongo import ASCENDING, DESCENDING, TEXT, IndexModel

from app.core.utils import utcnow


class Collaborator(BaseModel):
    """A co-owner of a mod or modpack. The primary creator (``owner_id``) adds
    collaborators, who then get full edit rights (everything except managing
    collaborators + deleting the project, which stay with the primary owner).
    ``username`` is denormalized for display + links."""

    user_id: PydanticObjectId                   # SiteUser.id
    username: str                               # denormalized handle (resynced on writes)


Visibility = Literal["draft", "unlisted", "public"]
ReleaseStatus = Literal["draft", "published"]
# Project mode. "files" = full versioned workflow (git commits, branches, clone)
# plus releases. "releases" = releases-only: the modder just uploads already-
# compiled builds; no file history / git / files view.
ProjectMode = Literal["files", "releases"]
# For "files" mode: whether the source (files view + git clone) is exposed
# publicly. "private" makes the hub an internal versioning tool - the source is
# owner-only and only the releases are public.
SourceVisibility = Literal["public", "private"]
# Compiled release artifact format (server-side compile from a commit).
ReleaseFormat = Literal["tmod", "zip"]


class ModProject(Document):
    """The repo metadata. File history lives in the project's git repo."""

    slug: str                                  # url token, unique PER OWNER (not global)
    title: str
    summary: str = ""                          # one-liner for cards
    description: str = ""                       # markdown readme
    # Long-form README for releases-only mode (no files to hold a README.md). In
    # files mode this is ignored - the repo's README.md is rendered instead.
    readme_text: str = ""
    # Highlighted warning blocks shown under the description; `<br>` splits blocks.
    warnings: str = ""

    # --- Creator-written translations -------------------------------------
    # Every piece of prose above can be written again in any language the site
    # speaks: `<field>_i18n` maps a language code ("fr", "de", … =
    # app.i18n.SUPPORTED minus "en") to that language's version. The base field
    # IS the English one and is always the fallback, so a partial translation
    # never leaves a blank. One switch on the mod page drives them all.
    #
    # `title_i18n` is DISPLAY ONLY: the URL slug, the .tmod's internal title and
    # every release filename stay on the base title (Trove matches a mod by that
    # exact name in-game). Files mode keeps its README translations in the repo
    # instead - README.<lang>.md next to README.md.
    title_i18n: dict[str, str] = Field(default_factory=dict)
    summary_i18n: dict[str, str] = Field(default_factory=dict)
    description_i18n: dict[str, str] = Field(default_factory=dict)
    readme_i18n: dict[str, str] = Field(default_factory=dict)
    warnings_i18n: dict[str, str] = Field(default_factory=dict)
    tags: list[str] = Field(default_factory=list)

    # The owning SiteUser. None ONLY for an imported *stray* mod (is_stray=True) -
    # an unclaimed mod mirrored from an external catalog that has no account behind
    # it yet. Set when the mod is handed over to a real user.
    owner_id: PydanticObjectId | None = None    # SiteUser.id (None for unclaimed stray mods)
    owner_username: str = ""                    # denormalized for display (may be a display name)
    # URL handle = the owner's canonical lowercase SiteUser.username. Mods are
    # addressed as /mods/<owner_handle>/<slug>, so the slug only has to be unique
    # within one owner. Refreshed to the current username on owner writes + when the
    # owner lists their mods, so renaming Discord moves their mods to the new handle.
    # Stray mods use the reserved handle "stray" -> /mods/stray/<slug>.
    owner_handle: str = ""
    # Co-owners (besides the primary owner). They get full edit rights; the primary
    # owner keeps collaborator management + deletion. See `Collaborator`.
    collaborators: list[Collaborator] = Field(default_factory=list)

    # --- Stray (imported, unclaimed) mods ---------------------------------
    # A stray mod is mirrored from an external catalog and not yet attributed to a
    # site user. It carries its original `author` name + the source it came from,
    # and can be *claimed* by a user (admin-approved) which assigns owner_id and
    # flips is_stray off (becoming an ordinary mod). Regular mods leave these unset.
    is_stray: bool = False
    # approved = visible in the public catalog; pending = mirrored but awaiting an
    # admin's approval (hidden); rejected = admin declined (hidden). The bulk import
    # creates mods "approved"; a later resync adds newly-found mods as "pending".
    stray_status: Literal["approved", "pending", "rejected"] | None = None
    author: str = ""                            # original author name (display)
    source: str | None = None                   # opaque import-source key (internal; never serialized out)
    source_id: str | None = None                # the mod's id in the source catalog (idempotent key)
    source_url: str | None = None               # link back to the source page (attribution)
    source_author_id: str | None = None         # the author's id in the source (future auto-match on claim)
    source_file_id: str | None = None           # the source file id we mirrored (detect newer files on resync)
    source_likes: int = 0                        # the source's like count (display only; our stars are separate)

    # --- Uploaded on behalf of someone else -------------------------------
    # An *uploaded* mod (as opposed to an *authored* one): a user shares a mod
    # they didn't make. It has a real owner_id (the uploader manages it, and it is
    # NOT claimable - distinct from a stray), but the creator credit points at a
    # named third party held in `author`. Forced into releases-only mode (you can't
    # own the source of code you didn't write) and its release artifacts must be
    # globally unique (anti-duplicate: the exact build can't already exist anywhere
    # on the hub). Displayed as "Uploaded by <owner> · Created by <author>".
    # Regular authored mods leave this False and `author` empty.
    uploaded_on_behalf: bool = False

    visibility: Visibility = "draft"
    mode: ProjectMode = "files"                 # files+releases vs releases-only
    source_visibility: SourceVisibility = "public"   # public source vs internal-tool
    default_branch: str = "main"

    banner_sha: str | None = None               # ModImageAsset.sha
    preview_shas: list[str] = Field(default_factory=list)

    # Owner-provided links shown on the mod page.
    discord_url: str | None = None              # the modder's own Discord invite
    website_url: str | None = None
    donation_urls: list[str] = Field(default_factory=list)   # up to 5 support links

    download_count: int = 0                     # sum across releases (denormalized)
    star_count: int = 0                          # users who starred (denormalized; see ModStar)
    # Popularity: recomputed periodically from ModDownloadEvent (7-day window) +
    # stars + total downloads, normalized to 0.0-1.0 across the public catalog.
    downloads_7d: int = 0                        # downloads in the trailing 7 days
    popularity_score: float = 0.0               # 0.0-1.0, top mod ~= 1.0

    # Branches are treated as mod *variants*: releases are grouped per branch and
    # the latest of each is surfaced. The owner can hide chosen variants' releases
    # from the public display (they still see them, flagged), and set the order the
    # variants appear in (branches not listed fall back to alphabetical at the end).
    hidden_release_branches: list[str] = Field(default_factory=list)
    branch_order: list[str] = Field(default_factory=list)

    # Attribution / lineage. A *fork* copies the original's files into this new
    # project and points back (forked_from_*); *inspired_by_* is a lighter
    # pointer with no content copy. Denormalized title/owner so the original is
    # always credited even if it's later renamed. fork_count tracks derivatives.
    forked_from_id: PydanticObjectId | None = None   # stable id of the original (fork listing)
    forked_from_slug: str | None = None
    forked_from_handle: str | None = None       # owner handle of the original (for the link)
    forked_from_title: str | None = None
    forked_from_owner: str | None = None
    inspired_by_slug: str | None = None
    inspired_by_handle: str | None = None
    inspired_by_title: str | None = None
    inspired_by_owner: str | None = None
    fork_count: int = 0

    # Master moderation. A taken-down project drops out of all public listings
    # and detail reads (the owner still sees it, flagged) until restored.
    taken_down: bool = False
    takedown_reason: str | None = None

    created_at: datetime = Field(default_factory=utcnow)
    updated_at: datetime = Field(default_factory=utcnow)

    class Settings:
        name = "mod_projects"
        indexes = [
            # Slug is unique PER OWNER now (was globally unique). The old global
            # slug_1 unique index is dropped on startup in database.py.
            IndexModel([("owner_id", ASCENDING), ("slug", ASCENDING)], unique=True),
            IndexModel([("owner_handle", ASCENDING), ("slug", ASCENDING)]),  # URL lookup
            IndexModel([("owner_id", ASCENDING), ("updated_at", DESCENDING)]),
            IndexModel([("collaborators.user_id", ASCENDING)]),   # "mods I collaborate on"
            IndexModel([("visibility", ASCENDING), ("updated_at", DESCENDING)]),
            IndexModel([("visibility", ASCENDING), ("download_count", DESCENDING)]),
            IndexModel([("visibility", ASCENDING), ("star_count", DESCENDING)]),
            IndexModel([("visibility", ASCENDING), ("popularity_score", DESCENDING)]),
            IndexModel([("tags", ASCENDING)]),
            IndexModel([("forked_from_id", ASCENDING)]),   # list a project's forks
            # Stray (imported) mods: idempotent upsert by source + dedup, and the
            # admin pending/approval queue. PARTIAL (source is a string) so regular
            # mods are excluded - sparse was WRONG: Beanie writes source=null (present,
            # not omitted), so a sparse unique index still indexes (null,null) and the
            # 2nd normal mod create collided -> DuplicateKeyError -> 500.
            IndexModel([("source", ASCENDING), ("source_id", ASCENDING)],
                       unique=True, partialFilterExpression={"source": {"$type": "string"}}),
            IndexModel([("is_stray", ASCENDING), ("stray_status", ASCENDING),
                        ("updated_at", DESCENDING)]),
            # Free-text search over the card-visible fields.
            IndexModel([("title", TEXT), ("summary", TEXT), ("tags", TEXT)],
                       name="mod_project_text"),
        ]


class ModRelease(Document):
    """A published build. ``tmod_sha`` is the compiled .tmod in the CAS."""

    project_id: PydanticObjectId
    owner_id: PydanticObjectId | None = None    # denormalized SiteUser.id (hash ownership + lookup)
    tag: str                                    # e.g. "v1.2.0" - unique per branch
    branch: str = "main"                         # the variant (branch) this release belongs to
    title: str = ""
    changelog: str = ""                          # markdown
    # The creator's translations of the two prose fields above, keyed by language
    # code (see ModProject's block). The tag and the artifact are never translated.
    title_i18n: dict[str, str] = Field(default_factory=dict)
    changelog_i18n: dict[str, str] = Field(default_factory=dict)

    source_commit_sha: str | None = None        # git commit it was compiled from
    release_format: ReleaseFormat = "tmod"       # .tmod or .zip artifact
    # The content hash (sha256 hex) of the artifact bytes - globally unique per
    # *owner*: a release whose hash is already owned by another creator is rejected.
    tmod_sha: str                                # CAS key of the artifact bytes
    # Every artifact hash this release has ALSO been, oldest first - filled when a
    # build is repacked to carry an attached config (ui/<title>.cfg): the modder's
    # own upload at release time, and whatever was published before an owner
    # attached a config to an existing release. A repack gives the release a new
    # `tmod_sha`, so these keep the copies already installed out there recognisable
    # to hash lookup (update detection), and keep the uniqueness guards from being
    # bypassed by adding a config to a known build.
    prior_tmod_shas: list[str] = Field(default_factory=list)
    tmod_size: int = 0
    tmod_filename: str = "mod.tmod"              # download name (carries the extension)
    # The header properties stamped into a .tmod (title/author/modVersion/…); empty for zips.
    tmod_properties: dict[str, str] = Field(default_factory=dict)

    banner_sha: str | None = None
    download_count: int = 0
    status: ReleaseStatus = "published"

    created_at: datetime = Field(default_factory=utcnow)
    updated_at: datetime = Field(default_factory=utcnow)
    published_at: datetime | None = None

    class Settings:
        name = "mod_releases"
        indexes = [
            # Tags are unique per *branch* (variant), not per project - branch X and
            # branch Y can each have their own v1.0 timeline. The old project-wide
            # unique index (project_id_1_tag_1) is dropped on startup in database.py.
            IndexModel([("project_id", ASCENDING), ("branch", ASCENDING), ("tag", ASCENDING)],
                       unique=True),
            IndexModel([("project_id", ASCENDING), ("published_at", DESCENDING)]),
            # Hash ownership check + the public lookup-by-hash API. Both hashes are
            # queried (a repacked build answers to its pre-injection hash too).
            IndexModel([("tmod_sha", ASCENDING)]),
            IndexModel([("prior_tmod_shas", ASCENDING)], sparse=True),   # multikey
        ]


class ModImageAsset(Document):
    """Sidecar for an image blob in the CAS - lets the serving route set the
    right Content-Type without re-sniffing the bytes on every request."""

    sha: str                                    # ContentStore key (unique)
    content_type: str                           # image/png | image/jpeg | image/webp | image/gif
    byte_size: int
    owner_id: PydanticObjectId | None = None    # uploader (None for images mirrored during a stray import)
    width: int | None = None                    # best-effort (None if not parsed)
    height: int | None = None
    # Rendered-on-demand downscales, `str(width) -> ContentStore key` (WebP).
    # The variant is itself a CAS blob, so this only records which one to serve.
    variants: dict[str, str] = Field(default_factory=dict)

    created_at: datetime = Field(default_factory=utcnow)

    class Settings:
        name = "mod_images"
        indexes = [IndexModel([("sha", ASCENDING)], unique=True)]


class ModGitToken(Document):
    """A personal access token a site user pastes as their git password to
    ``git clone/pull/push`` (Discord login has no password). Only the SHA-256 of
    the token is stored; the plaintext is shown once at creation."""

    site_user_id: PydanticObjectId
    token_hash: str                             # sha256(token) - unique
    prefix: str                                 # first chars, for display
    name: str = ""                              # user label
    revoked: bool = False
    created_at: datetime = Field(default_factory=utcnow)
    last_used_at: datetime | None = None

    class Settings:
        name = "mod_git_tokens"
        indexes = [
            IndexModel([("token_hash", ASCENDING)], unique=True),
            IndexModel([("site_user_id", ASCENDING), ("created_at", DESCENDING)]),
        ]


class ModCreatorLink(Document):
    """One connection between a **creator** (Dashboard ``SiteUser``) and a
    **developer's API account** (dev-portal ``User``), letting that account manage
    the creator's mods over the HTTP API.

    Established by the developer pasting the creator's creator token
    (``SiteUser.creator_token_hash``) into their portal account. The token only
    proves "this creator invited me" - the PERMISSIONS live here, on the link, and
    stay editable by the creator afterwards without re-issuing anything.

    A developer can hold many links (managing several creators); a creator can
    connect several developers. One row per (creator, developer) pair.

    Deliberately NOT covered by a link: deleting mods or releases, minting git
    tokens, and editing the creator's public profile. Those stay on the website,
    under the creator's own login."""

    site_user_id: PydanticObjectId              # the creator whose mods can be managed
    api_user_id: PydanticObjectId               # the dev-portal User granted access
    # The developer's own label for the connection ("release CI", "my bot"). It is
    # what the creator sees in their connections list, so it's how they tell two
    # connections apart when revoking one - we don't store the developer's email.
    label: str = ""

    # Scope. ``all_projects`` (the default a fresh connection gets) covers every
    # mod the creator owns INCLUDING ones created later - the "set it once" case.
    # Narrowing to a list also blocks creating new mods: a connection limited to
    # named mods must not be able to mint more.
    all_projects: bool = True
    project_ids: list[PydanticObjectId] = Field(default_factory=list)

    revoked: bool = False
    revoked_at: datetime | None = None
    # Set when the creator rotated their token: the link died with the token
    # rather than being individually revoked (surfaced differently in the UI).
    revoked_by_rotation: bool = False

    created_at: datetime = Field(default_factory=utcnow)
    last_used_at: datetime | None = None
    request_count: int = 0

    class Settings:
        name = "mod_creator_links"
        indexes = [
            IndexModel([("site_user_id", ASCENDING), ("api_user_id", ASCENDING)],
                       unique=True),
            # The per-request authorization lookup: this developer's live links.
            IndexModel([("api_user_id", ASCENDING), ("revoked", ASCENDING)]),
        ]


class ModStar(Document):
    """One user starring (favouriting) one project. Unique per (project, user);
    ``ModProject.star_count`` is the denormalized total for fast cards/sorting."""

    project_id: PydanticObjectId
    site_user_id: PydanticObjectId
    created_at: datetime = Field(default_factory=utcnow)

    class Settings:
        name = "mod_stars"
        indexes = [
            IndexModel([("project_id", ASCENDING), ("site_user_id", ASCENDING)], unique=True),
            IndexModel([("site_user_id", ASCENDING), ("created_at", DESCENDING)]),  # a user's stars
        ]


class ContentReport(Document):
    """A notice-and-action report against public user content - a mod, modpack, or
    creator profile - surfaced to masters for review/takedown (DSA Art. 16).

    Filed by ANYONE, including anonymously: we store only WHAT is reported and WHY,
    never who reported it (data minimization - no reporter identity is captured)."""

    target_type: Literal["mod", "modpack", "profile"]
    target_id: PydanticObjectId
    target_label: str = ""                       # display name/title, for triage
    target_url: str = ""                         # link the master opens to review
    reason: str
    resolved: bool = False

    created_at: datetime = Field(default_factory=utcnow)

    class Settings:
        name = "content_reports"
        indexes = [
            IndexModel([("resolved", ASCENDING), ("created_at", DESCENDING)]),
            IndexModel([("target_type", ASCENDING), ("target_id", ASCENDING)]),
        ]


class ModClaimRequest(Document):
    """A site user's request to claim a *stray* (imported, unowned) mod as their own.
    Surfaced to masters, who approve (hand the mod over - sets ``owner_id`` + clears
    ``is_stray``) or reject. One open request per (project, user)."""

    project_id: PydanticObjectId
    project_slug: str
    project_title: str = ""
    claimant_id: PydanticObjectId               # SiteUser.id
    claimant_username: str
    message: str = ""                            # optional note from the claimant ("proof"/context)
    status: Literal["pending", "approved", "rejected"] = "pending"
    resolved_by: PydanticObjectId | None = None  # master who approved/rejected
    resolved_at: datetime | None = None

    created_at: datetime = Field(default_factory=utcnow)

    class Settings:
        name = "mod_claim_requests"
        indexes = [
            IndexModel([("status", ASCENDING), ("created_at", DESCENDING)]),
            IndexModel([("project_id", ASCENDING)]),
            IndexModel([("claimant_id", ASCENDING), ("created_at", DESCENDING)]),
        ]


class StrayImportState(Document):
    """Singleton progress/state for the stray-mod bulk import + resync job, so the
    admin panel can show progress and the job is resumable (idempotent by source_id)."""

    key: str = "trovesaurus"                     # opaque singleton key (internal)
    running: bool = False
    phase: str = "idle"                          # idle | importing | resyncing | done | error
    total: int = 0                               # mods seen in the source catalog
    processed: int = 0                           # mods handled this run
    imported: int = 0                            # newly created this run
    updated: int = 0                             # existing refreshed this run
    pending_added: int = 0                       # new mods queued for approval (resync)
    failed: int = 0                              # mods that errored (skipped)
    last_error: str | None = None
    started_at: datetime | None = None
    finished_at: datetime | None = None
    updated_at: datetime = Field(default_factory=utcnow)

    class Settings:
        name = "stray_import_state"
        indexes = [IndexModel([("key", ASCENDING)], unique=True)]


class ModDownloadEvent(Document):
    """One download of one release - the raw signal behind the 7-day "popular"
    metric. Auto-expires after 8 days (a TTL index set in ``app/core/database.py``)
    so only the trailing window is kept; the lifetime totals live on the denormalized
    ``download_count`` fields."""

    project_id: PydanticObjectId
    release_id: PydanticObjectId
    created_at: datetime = Field(default_factory=utcnow)

    class Settings:
        name = "mod_download_events"
        indexes = [
            # NOTE: no plain {created_at:1} index here - the TTL index on created_at
            # is created in app/core/database.py (_ensure_ttl_index). Declaring both
            # collides (IndexOptionsConflict) and breaks startup.
            IndexModel([("project_id", ASCENDING), ("created_at", DESCENDING)]),
        ]


class ModProfile(Document):
    """A modder's customizable profile at ``/mods/<handle>``. One per site user
    (created lazily on first edit). The handle is the owner's ``SiteUser.username``
    (denormalized + resynced); resolution goes handle -> SiteUser -> this by
    ``site_user_id`` so it survives renames."""

    site_user_id: PydanticObjectId             # SiteUser.id (unique)
    handle: str = ""                           # denormalized username (URL key)

    display_name: str = ""                     # shown name (empty -> SiteUser display/username)
    tagline: str = ""                          # short one-liner under the name
    readme: str = ""                           # markdown "about"
    # The modder's own translations of their tagline + about text, keyed by
    # language code (see ModProject's block). The display name isn't translated.
    tagline_i18n: dict[str, str] = Field(default_factory=dict)
    readme_i18n: dict[str, str] = Field(default_factory=dict)

    avatar_sha: str | None = None              # custom profile picture (ModImageAsset sha)
    banner_sha: str | None = None              # profile banner

    discord_url: str | None = None
    website_url: str | None = None
    donation_urls: list[str] = Field(default_factory=list)   # up to 5

    # The order the modder's mods appear in (their slugs; not-listed go to the end
    # by recency) and a single highlighted mod shown in the sidebar.
    mod_order: list[str] = Field(default_factory=list)
    featured_slug: str | None = None

    # Master moderation - mirrors ModProject. A taken-down profile drops from public
    # view (the owner still sees it, flagged) until restored.
    taken_down: bool = False
    takedown_reason: str | None = None

    created_at: datetime = Field(default_factory=utcnow)
    updated_at: datetime = Field(default_factory=utcnow)

    class Settings:
        name = "mod_profiles"
        indexes = [
            IndexModel([("site_user_id", ASCENDING)], unique=True),
            IndexModel([("handle", ASCENDING)]),
        ]

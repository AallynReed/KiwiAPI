"""Mongo model for Modpacks.

One ``ModpackProject`` document holds the metadata plus embedded *variants* (each
an ordered list of mod *references*, never content). Images reuse the Mods Hub's
``ModImageAsset`` + shared CAS, so a banner is just a ``ModImageAsset.sha``.
"""

from datetime import datetime
from typing import Literal

from beanie import Document, PydanticObjectId
from pydantic import BaseModel, Field
from pymongo import ASCENDING, DESCENDING, TEXT, IndexModel

from app.core.utils import utcnow
from app.trove.mods_hub.models import Collaborator

Visibility = Literal["draft", "unlisted", "public"]


class ModpackEntry(BaseModel):
    """One mod inside a modpack variant. Two kinds:

    1. A **reference** to a Mods Hub mod (``project_id`` set) - tracked by its stable
       id (survives rename); ``handle``/``slug``/``title`` denormalized for display.
    2. A **custom uploaded** ``.tmod`` (``custom_sha`` set, ``project_id`` None) - a
       file the maker uploaded that isn't a hub mod. (If an upload's content hash
       matches a mod we already host, it's stored as a reference instead - never a
       duplicate file.)
    """

    project_id: PydanticObjectId | None = None  # ModProject.id (None for a custom upload)
    handle: str = ""                            # owner handle of the mod (denormalized; "" for custom)
    slug: str = ""                              # mod slug (denormalized; "" for custom)
    title: str = ""                             # title (denormalized mod title, or the uploaded .tmod's title)

    # Which *variant* (Mods Hub branch) of the mod was picked. Its releases are the
    # candidate builds for this entry. (Ignored for custom uploads.)
    branch: str = "main"

    # Version lock. OFF by default: the entry tracks the latest published build of
    # ``branch``. When ON, the entry is pinned to ``locked_tag`` (a release tag on
    # that branch) and never auto-updates - even if the mod ships a newer build.
    version_locked: bool = False
    locked_tag: str | None = None

    # Custom uploaded .tmod (kind 2): the CAS sha of the uploaded file + its download
    # name + author (read from the .tmod header). Empty for hub-mod references.
    custom_sha: str | None = None
    custom_filename: str | None = None
    author: str = ""


class ModpackVariant(BaseModel):
    """A spin-off of the pack: a named, ordered list of mod entries. Every pack has
    at least one (``default``). ``name`` is a slug unique within the pack."""

    name: str                                  # slug, unique within the pack (url token)
    label: str = ""                            # display name (empty -> name)
    entries: list[ModpackEntry] = Field(default_factory=list)


class ModpackProject(Document):
    """A modpack: metadata + embedded variants. No releases, no git, no files."""

    slug: str                                  # url token, unique PER OWNER
    title: str
    summary: str = ""                          # one-liner for cards
    description: str = ""                       # markdown readme
    # Highlighted warning blocks shown under the description; ``<br>`` splits blocks
    # (same convention as ModProject.warnings).
    warnings: str = ""
    tags: list[str] = Field(default_factory=list)

    owner_id: PydanticObjectId                  # SiteUser.id (primary owner)
    owner_username: str                         # denormalized for display
    owner_handle: str = ""                      # canonical lowercase username (URL handle)
    # Co-owners (besides the primary owner): full edit rights, but the primary owner
    # keeps collaborator management + deletion. See mods_hub Collaborator.
    collaborators: list[Collaborator] = Field(default_factory=list)

    visibility: Visibility = "draft"

    banner_sha: str | None = None               # ModImageAsset.sha (shared CAS)
    preview_shas: list[str] = Field(default_factory=list)

    discord_url: str | None = None
    website_url: str | None = None
    donation_urls: list[str] = Field(default_factory=list)   # up to 5 support links

    download_count: int = 0                     # total artifact downloads (denormalized)
    star_count: int = 0                          # users who liked it (denormalized; see ModpackStar)

    # Spin-offs. The first is the default unless ``default_variant`` names another;
    # display order is the list order.
    variants: list[ModpackVariant] = Field(default_factory=list)
    default_variant: str = "default"

    # Master moderation - mirrors ModProject. A taken-down pack drops from public
    # listings/reads (owner still sees it, flagged) until restored.
    taken_down: bool = False
    takedown_reason: str | None = None

    created_at: datetime = Field(default_factory=utcnow)
    updated_at: datetime = Field(default_factory=utcnow)

    class Settings:
        name = "modpack_projects"
        indexes = [
            IndexModel([("owner_id", ASCENDING), ("slug", ASCENDING)], unique=True),
            IndexModel([("owner_handle", ASCENDING), ("slug", ASCENDING)]),  # URL lookup
            IndexModel([("owner_id", ASCENDING), ("updated_at", DESCENDING)]),
            IndexModel([("collaborators.user_id", ASCENDING)]),   # "modpacks I collaborate on"
            IndexModel([("visibility", ASCENDING), ("updated_at", DESCENDING)]),
            IndexModel([("visibility", ASCENDING), ("download_count", DESCENDING)]),
            IndexModel([("visibility", ASCENDING), ("star_count", DESCENDING)]),
            IndexModel([("tags", ASCENDING)]),
            # Backlink: "which modpacks include this mod" - query by an embedded
            # entry's stable mod project id across all variants.
            IndexModel([("variants.entries.project_id", ASCENDING)]),
            IndexModel([("title", TEXT), ("summary", TEXT), ("tags", TEXT)],
                       name="modpack_project_text"),
        ]


class ModpackStar(Document):
    """One user's like of one modpack; unique per (modpack, user). The
    ``ModpackProject.star_count`` denormalized total is kept in sync separately.
    Mirrors the Mods Hub's ``ModStar``."""

    modpack_id: PydanticObjectId
    site_user_id: PydanticObjectId
    created_at: datetime = Field(default_factory=utcnow)

    class Settings:
        name = "modpack_stars"
        indexes = [
            IndexModel([("modpack_id", ASCENDING), ("site_user_id", ASCENDING)], unique=True),
            IndexModel([("site_user_id", ASCENDING), ("created_at", DESCENDING)]),  # a user's likes
        ]

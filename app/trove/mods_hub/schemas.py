"""Request bodies for the Mods Hub write API.

Read responses are returned as JSON-ready dicts straight from ``service.py``
(same style as the ``/site/updates/*`` proxies) so the website same-origin
proxies and the ``/v1/mods/hub/*`` API share one serialization path.
"""

from pydantic import BaseModel, Field

from app.trove.mods_hub.models import (
    ProjectMode,
    ReleaseFormat,
    ReleaseStatus,
    SourceVisibility,
    Visibility,
)


class CreateProjectRequest(BaseModel):
    title: str = Field(min_length=1, max_length=120)
    summary: str = Field(default="", max_length=280)
    description: str = Field(default="", max_length=40_000)
    tags: list[str] = Field(default_factory=list, max_length=12)
    visibility: Visibility = "draft"
    mode: ProjectMode = "files"
    source_visibility: SourceVisibility = "public"
    inspired_by: str | None = Field(
        default=None, max_length=80,
        description="Slug of an existing mod this one is inspired by (attribution only, no copy).",
    )
    on_behalf: bool = Field(
        default=False,
        description="This mod was made by someone else - upload it on their behalf. "
                    "Forces releases-only mode; the uploader owns it, `credited_author` is the creator credit.",
    )
    credited_author: str | None = Field(
        default=None, max_length=120,
        description="The original creator's name, shown as 'Created by …'. Required when on_behalf is true.",
    )


class UpdateProjectRequest(BaseModel):
    title: str | None = Field(default=None, min_length=1, max_length=120)
    summary: str | None = Field(default=None, max_length=280)
    description: str | None = Field(default=None, max_length=40_000)
    readme_text: str | None = Field(default=None, max_length=60_000,
                                    description="Long-form README for releases-only mode (ignored in files mode).")
    warnings: str | None = Field(default=None, max_length=4_000,
                                 description="Warning blocks shown under the description; <br> splits blocks.")
    tags: list[str] | None = Field(default=None, max_length=12)
    visibility: Visibility | None = None
    mode: ProjectMode | None = None
    source_visibility: SourceVisibility | None = None
    hidden_release_branches: list[str] | None = Field(
        default=None, max_length=100,
        description="Branches (variants) whose releases are hidden from the public display.",
    )
    branch_order: list[str] | None = Field(
        default=None, max_length=100,
        description="Display order of variants (branch names); unlisted branches fall to the end.",
    )
    discord_url: str | None = Field(default=None, max_length=300,
                                    description="Discord invite link; empty string clears it.")
    website_url: str | None = Field(default=None, max_length=300,
                                    description="Website link; empty string clears it.")
    donation_urls: list[str] | None = Field(
        default=None, max_length=5, description="Up to 5 support/donation links.")
    inspired_by: str | None = Field(
        default=None, max_length=80,
        description="Slug to credit as inspiration; empty string clears it.",
    )


class CreateBranchRequest(BaseModel):
    name: str = Field(min_length=1, max_length=80)
    from_ref: str | None = Field(
        default=None,
        description="Branch name or commit id to fork from; defaults to the default branch head.",
    )


class CreateReleaseRequest(BaseModel):
    """Compile-from-commit release. (The upload-a-.tmod path is multipart and
    handled directly in the router.)"""

    tag: str = Field(min_length=1, max_length=60)
    title: str = Field(default="", max_length=160)
    changelog: str = Field(default="", max_length=20_000)
    ref: str = Field(
        default="",
        description="Branch name or commit id to compile. Empty = default branch head.",
    )
    format: ReleaseFormat = "tmod"
    status: ReleaseStatus = "published"
    author: str | None = Field(
        default=None, max_length=200,
        description="Author(s) stamped into the .tmod (comma-separated for several); "
                    "defaults to the owner's name.",
    )
    preview_sha: str | None = Field(
        default=None,
        description="One of the project's preview images to embed in the .tmod as "
                    "ui/<slug>.<ext> (tmod format only; not committed to the repo).",
    )


class UpdateReleaseRequest(BaseModel):
    title: str | None = Field(default=None, max_length=160)
    changelog: str | None = Field(default=None, max_length=20_000)
    status: ReleaseStatus | None = None


class ReportRequest(BaseModel):
    reason: str = Field(min_length=3, max_length=2000)


class ClaimRequest(BaseModel):
    """A site user's request to claim a stray (imported) mod as their own."""

    message: str = Field(default="", max_length=2000,
                         description="Optional note to the admin (e.g. proof you're the author).")


class CollaboratorRequest(BaseModel):
    """Add a co-owner (collaborator) to a mod/modpack by their site username."""

    username: str = Field(min_length=1, max_length=80,
                          description="The site username to grant co-ownership to.")


class TakedownRequest(BaseModel):
    reason: str = Field(default="", max_length=2000)


class GitTokenRequest(BaseModel):
    name: str = Field(default="", max_length=60,
                      description="A label to recognise this token later (e.g. 'laptop').")


class UpdateProfileRequest(BaseModel):
    """Edit the signed-in modder's profile page (``/mods/<handle>``)."""

    display_name: str | None = Field(default=None, max_length=80)
    tagline: str | None = Field(default=None, max_length=160)
    readme: str | None = Field(default=None, max_length=40_000)
    discord_url: str | None = Field(default=None, max_length=300)
    website_url: str | None = Field(default=None, max_length=300)
    donation_urls: list[str] | None = Field(default=None, max_length=5)
    mod_order: list[str] | None = Field(default=None, max_length=500,
                                        description="Slugs in the order the mods should display.")
    featured_slug: str | None = Field(default=None, max_length=80,
                                      description="Slug of the highlighted mod (empty clears it).")


class HashLookupRequest(BaseModel):
    """Resolve mod metadata from one or more artifact content hashes."""

    hashes: list[str] = Field(
        min_length=1, max_length=200,
        description="Artifact sha256 hex hashes (one .tmod/.zip per hash) to resolve. Max 200.",
    )

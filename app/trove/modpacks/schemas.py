"""Request bodies for the Modpacks write API.

Read responses are JSON-ready dicts straight from ``service.py`` (same style as the
Mods Hub), shared by the ``/v1/modpacks/hub/*`` API and the ``/site/modpacks/*``
same-origin proxies.
"""

from pydantic import BaseModel, Field

from app.trove.modpacks.models import Visibility


class CreateModpackRequest(BaseModel):
    title: str = Field(min_length=1, max_length=120)
    summary: str = Field(default="", max_length=280)
    description: str = Field(default="", max_length=40_000)
    tags: list[str] = Field(default_factory=list, max_length=12)
    visibility: Visibility = "draft"


class UpdateModpackRequest(BaseModel):
    title: str | None = Field(default=None, min_length=1, max_length=120)
    summary: str | None = Field(default=None, max_length=280)
    description: str | None = Field(default=None, max_length=40_000)
    warnings: str | None = Field(default=None, max_length=4_000,
                                 description="Warning blocks shown under the description; <br> splits blocks.")
    tags: list[str] | None = Field(default=None, max_length=12)
    visibility: Visibility | None = None
    discord_url: str | None = Field(default=None, max_length=300,
                                    description="Discord invite link; empty string clears it.")
    website_url: str | None = Field(default=None, max_length=300,
                                    description="Website link; empty string clears it.")
    donation_urls: list[str] | None = Field(default=None, max_length=5,
                                            description="Up to 5 support/donation links.")
    default_variant: str | None = Field(default=None, max_length=80,
                                        description="Name of the variant shown/downloaded by default.")
    variant_order: list[str] | None = Field(default=None, max_length=100,
                                            description="Display order of variant names; unlisted fall to the end.")


class CreateVariantRequest(BaseModel):
    name: str = Field(min_length=1, max_length=80,
                      description="Variant label; slugified to a url-safe name unique within the pack.")
    copy_from: str | None = Field(default=None, max_length=80,
                                  description="Name of an existing variant to copy the mod list from (for a spin-off).")


class UpdateVariantRequest(BaseModel):
    label: str | None = Field(default=None, max_length=120)


class ModpackEntryInput(BaseModel):
    """One mod entry as the editor sends it. The server resolves the stable
    ``project_id`` + denormalized title from ``handle``/``slug``."""

    handle: str = Field(min_length=1, max_length=80)
    slug: str = Field(min_length=1, max_length=120)
    branch: str = Field(default="main", max_length=80,
                        description="Which variant (Mods Hub branch) of the mod to include.")
    version_locked: bool = Field(default=False,
                                 description="Pin to a specific version instead of tracking latest. Off by default.")
    locked_tag: str | None = Field(default=None, max_length=60,
                                   description="Release tag to pin to when version_locked is on.")


class SetEntriesRequest(BaseModel):
    """Replace a variant's full ordered mod list (add / remove / reorder / lock all
    in one write - the editor keeps the list in memory and PUTs the whole thing)."""

    entries: list[ModpackEntryInput] = Field(default_factory=list, max_length=300)


class TakedownRequest(BaseModel):
    reason: str = Field(default="", max_length=2000)

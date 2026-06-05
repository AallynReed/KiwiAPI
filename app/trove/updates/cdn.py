"""Trion update-CDN client + the three plaintext layer parsers.

Layers (all plain HTTP, no auth — see the protocol notes):
  1. bootstrap pointer  -> current version tag + content path        (per branch)
  2. versioned manifest -> the file set for that build (path, sha1, size)
  3. file               -> the on-disk bytes (write byte-for-byte; do NOT inflate)

The manifest `sha1` is opaque (not recomputable) — we use it ONLY as a per-file
"did this change?" key, never as a content hash. URL joins reproduce Glyph's
literal double slash after the prefix verbatim; the CDN accepts it.

The parsers here are pure and unit-tested; the network client is exercised only
on a live box (the first sync is multi-GB), never from CI.
"""

from __future__ import annotations

import urllib.parse

import httpx

# The timelines we track -> each branch's bootstrap-pointer filename. (PTS is
# region-less: kiwi-pts.txt, verified from a live capture — not kiwi-pts-us.txt.)
BRANCHES: dict[str, str] = {
    "live-us": "kiwi-live-us.txt",
    "pts": "kiwi-pts.txt",
}


class CdnError(ValueError):
    """Raised on a malformed pointer/manifest or a size mismatch."""


# --- Parsers (pure) --------------------------------------------------------


def parse_pointer(text: str) -> dict:
    """Pipe-delimited bootstrap pointer -> {version, content_path, motd, fields}.

    Field 1 arrives as `content/patchkiwi-live-us01`, but the manifest/file URL
    templates already carry their own `/content/` segment — so the bare path
    (`patchkiwi-live-us01`) is what we keep, to avoid a doubled `/content/content/`.
    """
    fields = text.strip().split("|")
    if len(fields) < 2 or not fields[0].strip() or not fields[1].strip():
        raise CdnError("malformed bootstrap pointer")
    return {
        "version": fields[0].strip(),
        "content_path": fields[1].strip().removeprefix("content/"),
        "motd": fields[3].strip() if len(fields) > 3 else "",
        "fields": fields,
    }


def parse_manifest(text: str) -> tuple[str, list[dict]]:
    """`version <tag>` + `path:sha1:size` lines -> (version, [{path, sha1, size}])."""
    lines = text.replace("\r\n", "\n").split("\n")
    if not lines or not lines[0].startswith("version "):
        raise CdnError("manifest missing 'version' line")
    version = lines[0].removeprefix("version ").strip()
    entries: list[dict] = []
    for line in lines[1:]:
        line = line.strip()
        if not line:
            continue
        try:
            path, sha1, size = line.rsplit(":", 2)  # rsplit: tolerate ':' in odd paths
            entries.append({"path": path.replace("\\", "/"), "sha1": sha1, "size": int(size)})
        except ValueError:
            continue  # skip a malformed line rather than abort the whole manifest
    return version, entries


# --- URL builders ----------------------------------------------------------


def pointer_url(base: str, prefix: str, pointer_file: str) -> str:
    return f"{base}{prefix}/public/{pointer_file}"


def manifest_url(base: str, prefix: str, content_path: str, version: str) -> str:
    return f"{base}{prefix}/content/{content_path}/{version}.manifest"


def file_url(base: str, prefix: str, content_path: str, path: str, sha1: str) -> str:
    quoted = urllib.parse.quote(path)  # keeps '/'; encodes spaces/specials
    return f"{base}{prefix}/content/{content_path}/recovery/{quoted}?sha1={sha1}"


# --- Async client ----------------------------------------------------------


class CdnClient:
    def __init__(self, base: str, prefix: str, *, timeout: float = 30.0,
                 user_agent: str = "KiwiAPI/1.0"):
        self._base = base
        self._prefix = prefix
        self._client = httpx.AsyncClient(timeout=timeout, headers={"User-Agent": user_agent})

    async def aclose(self) -> None:
        await self._client.aclose()

    async def __aenter__(self) -> CdnClient:
        return self

    async def __aexit__(self, *exc) -> None:
        await self.aclose()

    async def fetch_pointer(self, pointer_file: str) -> dict:
        r = await self._client.get(pointer_url(self._base, self._prefix, pointer_file))
        r.raise_for_status()
        return parse_pointer(r.text)

    async def fetch_manifest(self, content_path: str, version: str) -> tuple[str, list[dict]]:
        r = await self._client.get(manifest_url(self._base, self._prefix, content_path, version))
        r.raise_for_status()
        return parse_manifest(r.text)

    async def download_file(self, content_path: str, path: str, sha1: str,
                            expected_size: int | None = None) -> bytes:
        """Fetch one file's raw bytes. Verifies length against the manifest size."""
        url = file_url(self._base, self._prefix, content_path, path, sha1)
        r = await self._client.get(url)
        r.raise_for_status()
        data = r.content
        if expected_size is not None and len(data) != expected_size:
            raise CdnError(f"size mismatch for {path}: got {len(data)}, manifest {expected_size}")
        return data

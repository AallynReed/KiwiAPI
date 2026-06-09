# Handoff - Trove Update Archiver & Change-Comparing System

How to mirror Trove's update CDN, detect what changed between builds, and record
that diff. The reference implementation lives in `app/trove/updates/` - but nothing
here is tied to a particular web framework or host; it's the engine design.

---

## 1. TL;DR

A background worker probes Trion's update CDN for two timelines (`live-us`, `pts`)
every 20 min. When a build's file set changes it downloads **only the changed
files**, stores them content-addressed + deduped, and writes a per-file change log.
"Did it change?" is a cheap index diff; "what changed?" is a query over the change
log. The sync worker only runs when you switch it on.

---

## 2. Getting updates - the 3-layer CDN protocol

All plain HTTP, no auth. Base `http://trove-update.dyn.triongames.com`, prefix
`/kiwi-live-client-patch/`. URL builders + parsers in `app/trove/updates/cdn.py`.

| Layer | URL shape | Parsed into |
|---|---|---|
| **1. Bootstrap pointer** (per branch) | `…/public/<pointer_file>` | `{version, content_path, motd}` (pipe-delimited) |
| **2. Versioned manifest** | `…/content/<content_path>/<version>.manifest` | `version <tag>` + N× `path:sha1:size` lines |
| **3. File** | `…/content/<content_path>/recovery/<path>?sha1=<sha1>` | raw bytes |

Branch → pointer file: `live-us → kiwi-live-us.txt`, `pts → kiwi-pts.txt`
(PTS is **region-less** - not `kiwi-pts-us.txt`).

**Quirks:**
- The joined URL has an **intentional double slash** (`…patch//public/…`). The CDN
  accepts it; don't "fix" it.
- The manifest `sha1` is **opaque** - used only as a per-file *"did this change?"*
  key, never as a content hash.
- Layer-3 bytes are written **byte-for-byte**. A `.tfa` is itself a zlib stream we
  inflate later (at the archive layer), not here.

---

## 3. Detecting & comparing changes - two-level diff

Per probe, `sync_branch` in `app/trove/updates/ingest.py` runs:

### Level A - manifest diff (`classify_manifest_diff`, `diff.py`)
Compare the fresh manifest to our stored **sidecar** (`update_manifest`: the
last-seen opaque sha1 per top-level file). Output: changed/removed **loose files**
and which **archive directories** were touched.
**If nothing changed → `touch_probe` and stop; no version row is created.** This is
the normal every-20-min outcome.

### Level B - logical diff inside archives (`diff_logical`, `diff.py` + `archive.py`)
Most content is packed as `<dir>/index.tfi` + `archive0.tfa, archive1.tfa, …`
(format + reader detailed in **Appendix A**).
The `.tfi` is an index: each logical file → `(archive_index, offset, size,
FNV-1a hash)`. So we:
1. Download just the changed dir's `index.tfi`, parse it (`parse_tfi`).
2. Diff its **per-file FNV-1a hashes** against stored `update_state` → added /
   modified / removed logical files.
3. Download **only the `archiveN.tfa` holding a changed file**, inflate it
   (`decompress_tfa`, zlib), slice out **only** the changed files by
   `offset:offset+size`.

Net: a 95-file update pulls a handful of archives, not the whole tree.

**The change key is the TFI's per-file FNV-1a hash.** This is the crux - see §9.

---

## 4. Verifying integrity & dedup

- **Download size check** - `download_file` rejects any file whose byte length ≠ the
  manifest's declared size (`CdnError`).
- **FNV-1a content hash** - the `.tfi`'s per-file Trove FNV-1a hash is both the change
  key and integrity signal; `verify_entry(entry, data)` (size + `calculate_hash`)
  re-confirms extracted bytes.
- **Identity / dedup** - every stored blob is keyed by **SHA-256** in the CAS
  (`cas.py`), written atomically (temp + fsync + rename). Identical content collapses
  to one copy **across Live and PTS**; `bytes_added` counts only genuinely-new blobs.

---

## 5. What's persisted (Mongo - `models.py`)

| Collection | Role |
|---|---|
| `update_branches` | one per timeline: `current_version`, `current_ordinal`, `last_probe_at`, `status` (idle/syncing/error) |
| `update_versions` | one per detected build: `ordinal`, `version_tag`, counts (`files_added/modified/removed`, `bytes_added`), `status` (in_progress/complete) |
| `update_changes` | **append-only per-file change log** `(branch, ordinal, path) → type, content_sha256, fnv_hash, size`. **Source of truth for diffs** - what `/changes` reads |
| `update_state` | materialized **current** logical tree per branch (`path → sha256, fnv, size, archive`). This *is* the latest tree `/tree` and `/file` serve |
| `update_manifest` | the sidecar: last-seen opaque sha1 per top-level file (drives the Level-A diff) |

Writes are buffered + `bulk_write` (flush at 1000 ops), all idempotent
upserts/deletes (`repo.py`). Crash-safe resume: an `in_progress` version is reused
under the same ordinal; the manifest sidecar is committed **last** as the "done"
marker; `finish_version` flushes before marking `complete`.

---

## 6. Runtime & config (`worker.py`, `app/core/config.py`)

- A `_loop` starts in each uvicorn worker via the app lifespan; a **Redis leader
  lock** ensures exactly **one** worker syncs at a time (heartbeat renews it through
  the multi-GB first sync; if the holder dies the lock expires and another takes
  over). No Redis (dev / single worker) → it just runs.
- Knobs: `trove_update_enabled` (default **false**), `trove_update_probe_seconds`
  (**1200** = 20 min), `trove_update_concurrency` (**6** parallel downloads),
  `trove_update_store_dir` (`data/updates`, bind-mounted), `trove_update_base_url`,
  `trove_update_prefix`.
- ⚠️ First enable triggers a **multi-GB full sync** (every file logged as "added").

---

## 7. Reading the results

Everything you'd want to surface is a direct query over the collections in §5 - wrap
them in whatever interface you like (HTTP, CLI, a notebook). The natural reads:

| Question | Query |
|---|---|
| Current build + status per branch | `update_branches` |
| Version history, newest first, with delta counts | `update_versions` sorted by `-ordinal` |
| **Exactly what changed in a build** | `update_changes` filtered by `(branch, ordinal)` (optionally by `type`) |
| Browse / fetch a file in the current tree | `update_state` for the entry, then the CAS blob by its `content_sha256` |

Note the split: the **latest tree** comes from `update_state` (current snapshot),
while **full history** lives in `update_versions` + `update_changes` (every ordinal
is retained). Raw bytes always come from the CAS, addressed by `content_sha256`.

---

## 8. Worked example - PTS `TEST-103-3325-A-336166` (2026-06-08)

Probe at 14:54 UTC detected `…-335600` → `…-336166`: **7 added, 88 modified, 0
removed**, ingested in ~2s. The change log showed a **Steam + XIGNCODE3 anti-cheat
client overhaul** (new `steam_api64.dll`, `steam_appid.txt`=`304050`, three `.xem`
modules, new loader + crash handler, rebuilt `Trove_x64.exe`) plus all 87
`prefabs/plant/*.binfab` flagged modified. End-to-end: detection ✅, diff ✅,
extraction ✅.

---

## 9. Gotchas / invariants

- Opaque manifest `sha1` ≠ content hash. CAS **SHA-256** is the real identity; TFI
  **FNV-1a** is the per-logical-file change key.
- The CDN double slash is deliberate.
- A version row exists only when something changed; quiet probes just bump
  `last_probe_at`.
- The network client (`CdnClient`) is exercised only on a live box - keep CI on the
  **pure** parsers/diff (`parse_pointer/manifest/tfi`, `classify_manifest_diff`,
  `diff_logical`); they need no network and carry the correctness load.

---

## Appendix A - TFA/TFI archive format & reader (deep dive)

The pure-Python reader is `app/trove/updates/archive.py`; its binary primitives
(`read_leb128`, `calculate_hash`) live in `app/trove/troveio.py` and are shared with
the `.tmod` feature. The reader was ported from BetterTroveTools and its hash is
pinned byte-for-byte against the native `trove.dll`.

### A.1 Container layout

A CDN content directory (e.g. `prefabs/plant/`, `ui/`) holds:

```
prefabs/plant/index.tfi        ← the index (one per directory)
prefabs/plant/archive0.tfa     ← packed content blob 0
prefabs/plant/archive1.tfa     ← packed content blob 1
…
```

- **`.tfi` (Trove File Index)** - a flat list of logical-file entries. No bytes of
  actual content; just *where each logical file lives* and *a hash of it*.
- **`.tfa` (Trove File Archive)** - a single **zlib stream** whose inflated output is
  the **concatenation of the decompressed bytes** of every logical file assigned to
  that archive. Files are addressed by `(offset, size)` into this inflated buffer.

### A.2 The `.tfi` entry wire format

Every integer is **LEB128**, little-endian, read by `read_leb128` (`troveio.py`),
which **masks each value to 32 bits** (`& 0xFFFFFFFF`) to match Trove's own readers.
Each entry, in order (`parse_tfi`):

| Field | Encoding | Notes |
|---|---|---|
| `name_len` | leb128 | length of the name field in bytes |
| `name` | `name_len` raw bytes | UTF-8, **may be null-padded** → keep the prefix before the first `\x00`; backslashes → `/` (posix) |
| `archive_index` | leb128 | which `archiveN.tfa` holds this file |
| `offset` | leb128 | byte offset into that archive's **decompressed** content |
| `size` | leb128 | logical file size in bytes |
| `fnv_hash` | leb128 | Trove FNV-1a of the file's content - **the change key** |

`parse_tfi` loops over the whole buffer producing frozen `TfiEntry(name,
archive_index, offset, size, fnv_hash)` records. A truncated/garbled stream
(`IndexError`/`UnicodeDecodeError`/`ValueError`) raises `ArchiveError("malformed
.tfi: …")` - callers skip/400 rather than crash the sync.

### A.3 Reading a `.tfa`

```python
content = decompress_tfa(tfa_raw)        # zlib.decompressobj(wbits=MAX_WBITS)
data    = content[entry.offset : entry.offset + entry.size]
```

Because offsets index the **inflated** buffer, you must inflate the whole `.tfa`
once and then slice - you cannot seek into the compressed stream. A bad stream
raises `ArchiveError("could not inflate .tfa: …")`. Helpers:

- `entries_for_archive(entries, archive_index)` → the TFI entries in one archive.
- `slice_entries(content, entries)` → yields `(entry, bytes)` by `offset:size`.
- `extract_archive(tfa_raw, entries, archive_index)` → `{name: bytes}` for that
  archive. **Needs only this `.tfa` + the parsed `.tfi`** - sibling archives are
  never required.

### A.4 Integrity check - `verify_entry`

```python
verify_entry(entry, data) == (len(data) == entry.size
                              and calculate_hash(data) == entry.fnv_hash)
```

`calculate_hash` (`troveio.py`) is Trove's **FNV-1a-variant**, 32-bit unsigned,
verified against `trove.dll` (golden values in
`tests/unit/trove/test_tmod.py`):

- offset basis `2166136261`, prime `16777619`, all math `& 0xFFFFFFFF`;
- full 4-byte words are folded **little-endian, unsigned**;
- the trailing **1–3 bytes** are folded **big-endian and sign-extended** - a byte
  `≥ 0x80` is treated as a signed `char` and fills the upper 24 bits with 1s
  (the `_se` helper). **Do not "simplify" this tail handling** - it's what matches
  the native DLL.

This is the same hash the TFI stores per file, so an extracted slice can be
re-verified against its index entry end-to-end.

### A.5 Why this format makes the archiver cheap

Two properties (both confirmed against BetterTroveTools' reader):

1. **Index-level diffing.** The TFI carries a per-file hash, so two TFI versions diff
   by comparing `fnv_hash` per logical path - we learn *exactly which logical files
   changed* **without inflating a single archive** (`diff_logical` in `diff.py`).
2. **Per-archive extraction.** Every entry names its `archive_index`, so a single
   changed `archiveN.tfa` is fetched + inflated **alone** (download it + the `.tfi`;
   siblings aren't needed).

### A.6 How ingest wires it (`_sync_directory`, `ingest.py`)

For each archive directory the manifest flagged as touched:

1. Download just `index.tfi`; `parse_tfi` → `new_full = {logical_path: TfiEntry}`.
2. Build `new_fnv` (from the TFI) and `old_fnv` (from stored `update_state` via
   `get_archive_state`); `diff_logical(old_fnv, new_fnv)` → added / modified /
   removed.
3. Group the added+modified paths **by `archive_index`** → only those archives are
   downloaded.
4. Per archive: `download_file(archiveN.tfa)` → `decompress_tfa` (run in a thread) →
   slice `content[e.offset : e.offset + e.size]` for each changed file → store in the
   CAS → record the change.
5. Update the manifest sidecar for the touched `.tfa`/`.tfi`. If a directory's
   `index.tfi` vanished from the manifest, every logical file under it is recorded
   `removed`.

(The hot path slices directly, trusting the TFI's `offset/size`; `verify_entry` is
the available integrity helper if you want to harden it.)

### A.7 Code references

| What | Where |
|---|---|
| `TfiEntry`, `parse_tfi`, `decompress_tfa`, `extract_archive`, `verify_entry` | `app/trove/updates/archive.py` |
| `read_leb128`, `write_leb128`, `calculate_hash` (FNV-1a) | `app/trove/troveio.py` |
| `diff_logical`, `join_logical`, `parent_dir` | `app/trove/updates/diff.py` |
| read flow (`_sync_directory`, `_process_archive`) | `app/trove/updates/ingest.py` |
| hash golden-value tests | `tests/unit/trove/test_tmod.py` |

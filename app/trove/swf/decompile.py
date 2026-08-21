"""Recover the ActionScript source of a ``.swf`` by driving JPEXS FFDec.

A Trove interface mod is a Flash movie, and everything it actually *does* lives in
compiled AVM2 bytecode - so a mod page can list a ``.swf`` and hand it over, but
cannot show anyone what it does. FFDec is the decompiler that closes that gap: it
rebuilds real ActionScript 3 out of the bytecode, class by class.

It is a Java program, so this is a subprocess, and the input is a file a stranger
uploaded. Everything here exists to keep that bounded:

* the movie is refused outright past ``ffdec_max_swf_bytes`` - a decompiler's cost
  scales with the code inside, and the cheapest rejection is the one made first;
* the JVM runs headless, heap-capped, with its home pointed at the scratch
  directory, so a run leaves nothing behind and shares nothing with the next;
* FFDec's own three timeouts (per method, per file, per export) are set well under
  our wall-clock kill, so a method it cannot untangle degrades to P-code in the
  output instead of hanging the export;
* that wall-clock kill is the backstop, and it kills the *tree* - a JVM that
  ignored every timeout above still cannot outlive it;
* the exported tree is read back under a file-count and a byte cap, so a movie
  that decompiles into hundreds of megabytes of source cannot be served.

Nothing is executed: FFDec reads the bytecode, it does not run it.

The result is a pure function of the movie's bytes, which is what makes it worth
caching (see ``service.scripts``) - the same interface file shipped across twenty
releases is decompiled once.
"""

from __future__ import annotations

import logging
import os
import re
import shutil
import subprocess
import tempfile
from pathlib import Path

from app.core.config import settings
from app.trove.swf.extract import SwfError, read_header

logger = logging.getLogger("kiwi.swf")

# Where a script export lands under the output directory FFDec is given.
_EXPORT_SUBDIR = "scripts"

# Install locations worth probing when nothing is configured, so a dev box with
# FFDec already on it works without an .env entry. The configured path wins.
_FALLBACK_JARS = (
    "/opt/ffdec/ffdec.jar",
    "/usr/share/ffdec/ffdec.jar",
    r"C:\Program Files (x86)\FFDec\ffdec-cli.jar",
    r"C:\Program Files\FFDec\ffdec-cli.jar",
)

_VERSION_RE = re.compile(r"Free Flash Decompiler\s+v\.?\s*([\d.]+)")


class DecompileError(Exception):
    """The movie could not be decompiled."""


class DecompilerUnavailable(DecompileError):
    """FFDec (or a JVM to run it in) is not installed on this box."""


def _find_jar() -> str | None:
    configured = (settings.ffdec_jar or "").strip()
    for cand in ((configured,) if configured else ()) + _FALLBACK_JARS:
        if cand and os.path.isfile(cand):
            return cand
    return None


def _find_java() -> str | None:
    return shutil.which(settings.ffdec_java or "java")


def available() -> bool:
    """Whether this box can decompile at all - both halves present."""
    return bool(_find_jar() and _find_java())


def _config_flags() -> str:
    """FFDec's own bounds, passed per-run so nothing is written to its config.

    ``parallelSpeedUp`` is left on but pinned to two threads: the export of a big
    movie is embarrassingly parallel and halves with it, while an unpinned FFDec
    would size its pool to every core on the host.
    """
    return ",".join((
        f"decompilationTimeoutSingleMethod={settings.ffdec_method_timeout}",
        f"decompilationTimeoutFile={settings.ffdec_file_timeout}",
        f"exportTimeout={settings.ffdec_timeout}",
        "parallelSpeedUp=true",
        "parallelSpeedUpThreadCount=2",
        "autoDeobfuscate=true",
    ))


def _read_export(root: Path) -> tuple[list[dict], bool]:
    """Read the exported tree back as ``[{path, size, source}]``.

    Paths are relative to the export root and slash-separated, so they read as the
    package they came from (``_kiwi/Core/UIComponent.as``). Sorted by path, which
    puts a package's classes together.
    """
    out: list[dict] = []
    total = 0
    truncated = False
    files = sorted((p for p in root.rglob("*") if p.is_file()),
                   key=lambda p: str(p.relative_to(root)).replace("\\", "/").lower())
    for path in files:
        if len(out) >= settings.ffdec_max_scripts:
            truncated = True
            break
        try:
            data = path.read_bytes()
        except OSError:
            continue
        total += len(data)
        if total > settings.ffdec_max_source_bytes:
            truncated = True
            break
        out.append({
            "path": str(path.relative_to(root)).replace("\\", "/"),
            "size": len(data),
            "source": data.decode("utf-8", "replace"),
        })
    return out, truncated


def decompile_scripts(raw: bytes) -> dict:
    """Every ActionScript class in ``raw``, decompiled.

    Blocking (a subprocess and a tree of file reads) - call it in a thread. Returns
    ``{scripts, count, truncated, decompiler}``; ``scripts`` is empty for a movie
    that is pure artwork, which is a real answer, not a failure.
    """
    jar, java = _find_jar(), _find_java()
    if not jar or not java:
        raise DecompilerUnavailable("The Flash decompiler is not installed here.")
    if len(raw) > settings.ffdec_max_swf_bytes:
        raise DecompileError("This movie is too large to decompile.")
    # Check the container ourselves first. FFDec answers "no scripts" for a file
    # that isn't a movie at all, which would read as "this mod has no code" -
    # and our own reader settles it without spending a JVM to find out.
    try:
        read_header(raw)
    except SwfError as exc:
        raise DecompileError(f"This file could not be read as a Flash movie ({exc}).") from None

    work = tempfile.mkdtemp(prefix="ffdec-")
    try:
        movie = os.path.join(work, "movie.swf")
        outdir = os.path.join(work, "out")
        with open(movie, "wb") as fh:
            fh.write(raw)

        cmd = [
            java,
            f"-Xmx{settings.ffdec_max_heap_mb}m",
            "-Djava.awt.headless=true",
            f"-Duser.home={work}",          # keep FFDec's own state inside the scratch dir
            "-jar", jar,
            "-config", _config_flags(),
            "-export", "script", outdir, movie,
        ]
        env = {**os.environ, "HOME": work, "USERPROFILE": work}
        try:
            proc = subprocess.run(                      # noqa: S603 - fixed argv, no shell
                cmd, capture_output=True, timeout=settings.ffdec_timeout + 30,
                env=env, cwd=work, check=False,
            )
        except subprocess.TimeoutExpired:
            logger.warning("swf: decompile timed out after %ss", settings.ffdec_timeout + 30)
            raise DecompileError("This movie took too long to decompile.") from None
        except OSError as exc:
            raise DecompilerUnavailable(str(exc)) from None

        stdout = proc.stdout.decode("utf-8", "replace")
        root = Path(outdir) / _EXPORT_SUBDIR
        if not root.is_dir():
            # No script tree at all: either the movie carries no code (fine, and
            # common for pure-art mods) or FFDec refused it (not fine).
            if proc.returncode != 0:
                logger.info("swf: ffdec exited %s: %s", proc.returncode,
                            proc.stderr.decode("utf-8", "replace")[:400])
                raise DecompileError("This file could not be read as a Flash movie.")
            return {"scripts": [], "count": 0, "truncated": False,
                    "decompiler": _version(stdout)}

        scripts, truncated = _read_export(root)
        return {"scripts": scripts, "count": len(scripts), "truncated": truncated,
                "decompiler": _version(stdout)}
    finally:
        shutil.rmtree(work, ignore_errors=True)


def _version(stdout: str) -> str:
    m = _VERSION_RE.search(stdout)
    return f"ffdec {m.group(1)}" if m else "ffdec"

"""Request bodies for the mods_git twin spec: git-receive-pack pushes and a
git-upload-pack fetch, built with the local git so their objects are fixed.
    python native/port/parity/gen_mods_git_push_bodies.py
Writes native/port/parity/data/mods_git/*.bin and prints the commit ids.
"""
import os
import shutil
import subprocess
import tempfile
from pathlib import Path

OUT = Path(__file__).resolve().parent / "data" / "mods_git"
ZERO = "0" * 40
ENV = dict(os.environ, GIT_AUTHOR_NAME="Parity Pusher", GIT_AUTHOR_EMAIL="pusher@users.noreply.kiwi",
           GIT_COMMITTER_NAME="Parity Pusher", GIT_COMMITTER_EMAIL="pusher@users.noreply.kiwi",
           GIT_AUTHOR_DATE="1700000000 +0000", GIT_COMMITTER_DATE="1700000000 +0000",
           GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_NOSYSTEM="1")


def git(repo, *args, stdin=None):
    return subprocess.run(["git", *args], cwd=repo, env=ENV, input=stdin, capture_output=True, check=True).stdout


def pkt(line: bytes) -> bytes:
    return b"%04x" % (len(line) + 4) + line


def commit(repo, files: dict[str, bytes], message: str) -> str:
    for path, data in files.items():
        p = Path(repo) / path
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_bytes(data)
    git(repo, "add", "-A")
    git(repo, "commit", "-q", "-m", message)
    return git(repo, "rev-parse", "HEAD").decode().strip()


def push_body(repo, ref: str, sha: str) -> bytes:
    pack = git(repo, "pack-objects", "--stdout", "--revs", "-q", stdin=f"{sha}\n".encode())
    return pkt(f"{ZERO} {sha} {ref}\0report-status\n".encode()) + b"0000" + pack


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    tmp = tempfile.mkdtemp(prefix="kiwi-mods-git-push-")
    try:
        clean, blocked = Path(tmp, "clean"), Path(tmp, "blocked")
        for r in (clean, blocked):
            r.mkdir()
            git(r, "init", "-q", "-b", "main")
        ok = commit(clean, {"ui/pushed.txt": b"pushed over git\n", "README.md": b"# Pushed\n"}, "pushed commit")
        bad = commit(blocked, {"ui/a.txt": b"a\n", "Built.TMOD": b"not a source file\n"}, "blocked commit")
        (OUT / "push_ok.bin").write_bytes(push_body(clean, "refs/heads/pushed", ok))
        (OUT / "push_blocked.bin").write_bytes(push_body(blocked, "refs/heads/blocked", bad))
        (OUT / "fetch.bin").write_bytes(pkt(f"want {ok} side-band-64k thin-pack no-progress ofs-delta\n".encode()) + b"0000" + pkt(b"done\n"))
        print(f"pushed={ok} blocked={bad}")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    main()

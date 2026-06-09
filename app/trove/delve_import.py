"""One-off importer: load a directory of ``<week>.json`` delve files into Mongo.

Each file is a source payload (``{depths, total, …}``) named by its week id.
Lives under ``app/`` so it's available in the running container via the source
bind-mount (no image rebuild). Run it where Mongo is reachable - easiest is inside
the api container, which already has ``MONGO_URI`` wired:

    docker compose cp ./delves api:/tmp/delves          # get the files into the container
    docker compose exec api python -m app.trove.delve_import /tmp/delves

Idempotent: re-running upserts each week.
"""

import asyncio
import json
import sys
from pathlib import Path

from app.core.database import close_db, init_db
from app.trove.delves import store_week


async def import_directory(directory: str) -> None:
    files = sorted(
        (p for p in Path(directory).glob("*.json") if p.stem.isdigit()),
        key=lambda p: int(p.stem),
    )
    if not files:
        print(f"No <week>.json files found in {directory}")
        return
    await init_db()
    try:
        total = 0
        for f in files:
            week = int(f.stem)
            try:
                payload = json.loads(f.read_text(encoding="utf-8"))
            except (OSError, json.JSONDecodeError) as e:
                print(f"week {week}: SKIPPED ({e})")
                continue
            count = await store_week(week, payload)
            total += count
            print(f"week {week}: {count} depths")
        print(f"Done - imported {len(files)} week(s), {total} depth records.")
    finally:
        await close_db()


if __name__ == "__main__":
    if len(sys.argv) != 2:
        print("usage: python -m app.trove.delve_import <directory>")
        raise SystemExit(2)
    asyncio.run(import_directory(sys.argv[1]))

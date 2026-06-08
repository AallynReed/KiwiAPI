"""One-shot: add the 'Open this board in the Leaderboards view' tooltip
to each locale JSON. Additive — existing keys are preserved.

Run: `python scripts/merge_leaderboards_goto_board.py`
"""
from __future__ import annotations

import json
from pathlib import Path

KEY = "Open this board in the Leaderboards view"

TRANSLATIONS: dict[str, str] = {
    "fr": "Ouvrir ce classement dans la vue Classements",
    "de": "Diese Liste in der Bestenlisten-Ansicht öffnen",
    "pt-PT": "Abrir esta classificação na vista Classificações",
    "ru": "Открыть эту таблицу в разделе таблиц",
    "ja": "このボードをランキング画面で開く",
    "zh-CN": "在排行榜视图中打开此榜单",
}


def merge() -> None:
    locales_dir = Path("site/static/locales")
    for lang, value in TRANSLATIONS.items():
        target = locales_dir / f"{lang}.json"
        if not target.exists():
            print(f"  skip {lang} (no file)")
            continue
        with target.open("r", encoding="utf-8") as f:
            existing = json.load(f)
        if KEY in existing:
            print(f"  {lang}: already has key")
            continue
        existing[KEY] = value
        with target.open("w", encoding="utf-8") as f:
            json.dump(existing, f, ensure_ascii=False, indent=2, sort_keys=True)
            f.write("\n")
        print(f"  {lang}: added (total now {len(existing)})")


if __name__ == "__main__":
    merge()

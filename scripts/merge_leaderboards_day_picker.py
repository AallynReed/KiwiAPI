"""One-shot: add the day-picker chrome strings to each locale JSON.
Additive — existing keys are preserved.

Run: `python scripts/merge_leaderboards_day_picker.py`
"""
from __future__ import annotations

import json
from pathlib import Path

TRANSLATIONS: dict[str, dict[str, str]] = {
    "fr": {
        "Day": "Jour",
        "Today": "Aujourd'hui",
        "Yesterday": "Hier",
        "No data": "Aucune donnée",
    },
    "de": {
        "Day": "Tag",
        "Today": "Heute",
        "Yesterday": "Gestern",
        "No data": "Keine Daten",
    },
    "pt-PT": {
        "Day": "Dia",
        "Today": "Hoje",
        "Yesterday": "Ontem",
        "No data": "Sem dados",
    },
    "ru": {
        "Day": "День",
        "Today": "Сегодня",
        "Yesterday": "Вчера",
        "No data": "Нет данных",
    },
    "ja": {
        "Day": "日",
        "Today": "今日",
        "Yesterday": "昨日",
        "No data": "データなし",
    },
    "zh-CN": {
        "Day": "日期",
        "Today": "今天",
        "Yesterday": "昨天",
        "No data": "无数据",
    },
}


def merge() -> None:
    locales_dir = Path("site/static/locales")
    for lang, new_pairs in TRANSLATIONS.items():
        target = locales_dir / f"{lang}.json"
        if not target.exists():
            print(f"  skip {lang} (no existing file)")
            continue
        with target.open("r", encoding="utf-8") as f:
            existing = json.load(f)
        added = 0
        for k, v in new_pairs.items():
            if k not in existing:
                existing[k] = v
                added += 1
        with target.open("w", encoding="utf-8") as f:
            json.dump(existing, f, ensure_ascii=False, indent=2, sort_keys=True)
            f.write("\n")
        print(f"  {lang}: added {added} keys (total now {len(existing)})")


if __name__ == "__main__":
    merge()

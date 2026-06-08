"""One-shot: add the activity-pill chrome strings to each locale JSON.
Additive — existing keys are preserved.

Run: `python scripts/merge_leaderboards_activity.py`
"""
from __future__ import annotations

import json
from pathlib import Path

TRANSLATIONS: dict[str, dict[str, str]] = {
    "fr": {
        "~{n} active players in the last hour": "~{n} joueurs actifs sur la dernière heure",
        "~{n} active players in the last {h}h": "~{n} joueurs actifs sur les dernières {h}h",
    },
    "de": {
        "~{n} active players in the last hour": "~{n} aktive Spieler in der letzten Stunde",
        "~{n} active players in the last {h}h": "~{n} aktive Spieler in den letzten {h}h",
    },
    "pt-PT": {
        "~{n} active players in the last hour": "~{n} jogadores ativos na última hora",
        "~{n} active players in the last {h}h": "~{n} jogadores ativos nas últimas {h}h",
    },
    "ru": {
        "~{n} active players in the last hour": "~{n} активных игроков за последний час",
        "~{n} active players in the last {h}h": "~{n} активных игроков за последние {h}ч",
    },
    "ja": {
        "~{n} active players in the last hour": "直近 1 時間でアクティブだったプレイヤー約 {n} 名",
        "~{n} active players in the last {h}h": "直近 {h} 時間でアクティブだったプレイヤー約 {n} 名",
    },
    "zh-CN": {
        "~{n} active players in the last hour": "过去 1 小时约 {n} 名活跃玩家",
        "~{n} active players in the last {h}h": "过去 {h} 小时约 {n} 名活跃玩家",
    },
}


def merge() -> None:
    locales_dir = Path("site/static/locales")
    for lang, new_pairs in TRANSLATIONS.items():
        target = locales_dir / f"{lang}.json"
        if not target.exists():
            print(f"  skip {lang} (no file)")
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

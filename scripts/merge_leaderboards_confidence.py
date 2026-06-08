"""One-shot: add the confidence-filter chrome strings to each locale
JSON. Additive — existing keys are preserved.

Run: `python scripts/merge_leaderboards_confidence.py`
"""
from __future__ import annotations

import json
from pathlib import Path

TRANSLATIONS: dict[str, dict[str, str]] = {
    "fr": {
        "Min confidence": "Confiance min",
        "Confidence": "Confiance",
        "hiding {n} below threshold": "{n} masqué(s) sous le seuil",
        "All {f} flagged player(s) are below the current confidence threshold — slide left to see them.": "Les {f} joueur(s) signalé(s) sont sous le seuil de confiance actuel — glissez à gauche pour les voir.",
    },
    "de": {
        "Min confidence": "Min. Konfidenz",
        "Confidence": "Konfidenz",
        "hiding {n} below threshold": "{n} unter Schwelle ausgeblendet",
        "All {f} flagged player(s) are below the current confidence threshold — slide left to see them.": "Alle {f} markierten Spieler liegen unter der aktuellen Konfidenzschwelle — Regler nach links für Anzeige.",
    },
    "pt-PT": {
        "Min confidence": "Confiança mín",
        "Confidence": "Confiança",
        "hiding {n} below threshold": "{n} ocultado(s) abaixo do limite",
        "All {f} flagged player(s) are below the current confidence threshold — slide left to see them.": "Todos os {f} jogadore(s) sinalizados estão abaixo do limite atual — deslize para a esquerda para os ver.",
    },
    "ru": {
        "Min confidence": "Мин. достоверность",
        "Confidence": "Достоверность",
        "hiding {n} below threshold": "Скрыто ниже порога: {n}",
        "All {f} flagged player(s) are below the current confidence threshold — slide left to see them.": "Все отмеченные игроки ({f}) — ниже текущего порога достоверности. Сдвиньте ползунок влево, чтобы их увидеть.",
    },
    "ja": {
        "Min confidence": "最小信頼度",
        "Confidence": "信頼度",
        "hiding {n} below threshold": "{n} 件をしきい値以下で非表示",
        "All {f} flagged player(s) are below the current confidence threshold — slide left to see them.": "検出された {f} 名はすべて現在の信頼度しきい値未満です — スライダーを左に動かすと表示されます。",
    },
    "zh-CN": {
        "Min confidence": "最低置信度",
        "Confidence": "置信度",
        "hiding {n} below threshold": "{n} 项低于阈值已隐藏",
        "All {f} flagged player(s) are below the current confidence threshold — slide left to see them.": "全部 {f} 位被标记玩家均低于当前置信度阈值 — 向左滑动滑块以查看。",
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

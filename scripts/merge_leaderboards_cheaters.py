"""One-shot: add the Possible-cheaters panel chrome strings to each
locale JSON. Additive — existing keys are preserved.

Run: `python scripts/merge_leaderboards_cheaters.py`
"""
from __future__ import annotations

import json
from pathlib import Path

TRANSLATIONS: dict[str, dict[str, str]] = {
    "fr": {
        "Possible cheaters": "Tricheurs possibles",
        "Checking the latest capture…": "Analyse de la dernière capture…",
        "No suspicious activity flagged.": "Aucune activité suspecte signalée.",
        "How is this computed?": "Comment c'est calculé ?",
        "Three independent statistical checks per board: Modified Z-score (median-based, robust to cheaters polluting their own baseline), rank-gap ratio (a player's lead over the next rank vs the board's typical between-rank step), and velocity (score gain rate vs the board's peer p95). A player flagged by multiple checks, or across multiple boards, is higher confidence.": "Trois vérifications statistiques indépendantes par classement : Z-score modifié (basé sur la médiane, résistant aux tricheurs qui polluent leur propre référence), ratio d'écart de rang (l'avance d'un joueur sur le rang suivant vs l'écart typique entre rangs), et vélocité (taux de gain de score vs le p95 des pairs). Un joueur signalé par plusieurs vérifications, ou sur plusieurs classements, est plus suspect.",
        "Flagged {n} player(s) across {b} board(s) — based on the capture from {when}.": "{n} joueur(s) signalé(s) sur {b} classement(s) — d'après la capture du {when}.",
        "Scanned {b} board(s) from the capture at {when} — nothing anomalous.": "Analysé {b} classement(s) de la capture du {when} — rien d'anormal.",
        "No capture available yet to analyse.": "Aucune capture disponible à analyser.",
        "{n} board(s)": "{n} classement(s)",
        "{n} flag(s)": "{n} signalement(s)",
        "View history": "Voir l'historique",
        "Score outlier": "Score aberrant",
        "Rank gap": "Écart de rang",
        "Velocity": "Vélocité",
    },
    "de": {
        "Possible cheaters": "Mögliche Cheater",
        "Checking the latest capture…": "Letzte Erfassung wird geprüft…",
        "No suspicious activity flagged.": "Keine verdächtige Aktivität markiert.",
        "How is this computed?": "Wie wird das berechnet?",
        "Three independent statistical checks per board: Modified Z-score (median-based, robust to cheaters polluting their own baseline), rank-gap ratio (a player's lead over the next rank vs the board's typical between-rank step), and velocity (score gain rate vs the board's peer p95). A player flagged by multiple checks, or across multiple boards, is higher confidence.": "Drei unabhängige statistische Prüfungen pro Liste: Modifizierter Z-Score (medianbasiert, robust gegen Cheater, die ihre eigene Basis verfälschen), Rang-Lücken-Verhältnis (der Vorsprung eines Spielers vor dem nächsten Rang gegenüber dem typischen Rangabstand der Liste) und Geschwindigkeit (Punktzuwachsrate gegenüber dem p95 der Mitspieler). Ein Spieler, der durch mehrere Prüfungen oder auf mehreren Listen markiert wird, ist mit höherer Konfidenz verdächtig.",
        "Flagged {n} player(s) across {b} board(s) — based on the capture from {when}.": "{n} Spieler auf {b} Listen markiert — basierend auf der Erfassung vom {when}.",
        "Scanned {b} board(s) from the capture at {when} — nothing anomalous.": "{b} Listen aus der Erfassung vom {when} geprüft — nichts Ungewöhnliches.",
        "No capture available yet to analyse.": "Noch keine Erfassung zur Analyse verfügbar.",
        "{n} board(s)": "{n} Liste(n)",
        "{n} flag(s)": "{n} Markierung(en)",
        "View history": "Verlauf ansehen",
        "Score outlier": "Punkte-Ausreißer",
        "Rank gap": "Ranglücke",
        "Velocity": "Geschwindigkeit",
    },
    "pt-PT": {
        "Possible cheaters": "Possíveis batoteiros",
        "Checking the latest capture…": "A verificar a última captura…",
        "No suspicious activity flagged.": "Nenhuma atividade suspeita sinalizada.",
        "How is this computed?": "Como é calculado?",
        "Three independent statistical checks per board: Modified Z-score (median-based, robust to cheaters polluting their own baseline), rank-gap ratio (a player's lead over the next rank vs the board's typical between-rank step), and velocity (score gain rate vs the board's peer p95). A player flagged by multiple checks, or across multiple boards, is higher confidence.": "Três verificações estatísticas independentes por classificação: Z-score modificado (baseado na mediana, robusto a batoteiros que poluem a sua própria base), rácio de diferença de posição (a vantagem de um jogador sobre a posição seguinte vs o passo típico entre posições) e velocidade (taxa de ganho de pontuação vs o p95 dos pares). Um jogador sinalizado por várias verificações, ou em várias classificações, tem confiança superior.",
        "Flagged {n} player(s) across {b} board(s) — based on the capture from {when}.": "Sinalizado(s) {n} jogador(es) em {b} classificação(ões) — com base na captura de {when}.",
        "Scanned {b} board(s) from the capture at {when} — nothing anomalous.": "Analisada(s) {b} classificação(ões) da captura de {when} — nada anómalo.",
        "No capture available yet to analyse.": "Sem captura disponível para analisar.",
        "{n} board(s)": "{n} classificação(ões)",
        "{n} flag(s)": "{n} sinalização(ões)",
        "View history": "Ver histórico",
        "Score outlier": "Pontuação anómala",
        "Rank gap": "Salto de posição",
        "Velocity": "Velocidade",
    },
    "ru": {
        "Possible cheaters": "Возможные читеры",
        "Checking the latest capture…": "Проверка последнего снимка…",
        "No suspicious activity flagged.": "Подозрительной активности не выявлено.",
        "How is this computed?": "Как это вычисляется?",
        "Three independent statistical checks per board: Modified Z-score (median-based, robust to cheaters polluting their own baseline), rank-gap ratio (a player's lead over the next rank vs the board's typical between-rank step), and velocity (score gain rate vs the board's peer p95). A player flagged by multiple checks, or across multiple boards, is higher confidence.": "Три независимые статистические проверки по каждой таблице: модифицированный Z-показатель (на основе медианы, устойчив к попыткам читеров исказить свою базу), отношение разрыва между рангами (отрыв игрока от следующего ранга против типичного шага между рангами таблицы) и скорость (темп прироста очков против p95 пиров). Игрок, помеченный несколькими проверками или сразу на нескольких таблицах, имеет более высокую достоверность.",
        "Flagged {n} player(s) across {b} board(s) — based on the capture from {when}.": "Отмечено игроков: {n} на таблицах: {b} — по снимку от {when}.",
        "Scanned {b} board(s) from the capture at {when} — nothing anomalous.": "Проверено таблиц: {b} из снимка {when} — аномалий не обнаружено.",
        "No capture available yet to analyse.": "Снимок для анализа пока недоступен.",
        "{n} board(s)": "Таблиц: {n}",
        "{n} flag(s)": "Сигналов: {n}",
        "View history": "Открыть историю",
        "Score outlier": "Аномалия очков",
        "Rank gap": "Разрыв ранга",
        "Velocity": "Скорость",
    },
    "ja": {
        "Possible cheaters": "チート疑いプレイヤー",
        "Checking the latest capture…": "最新のキャプチャを確認中…",
        "No suspicious activity flagged.": "怪しい挙動はありません。",
        "How is this computed?": "どのように計算していますか？",
        "Three independent statistical checks per board: Modified Z-score (median-based, robust to cheaters polluting their own baseline), rank-gap ratio (a player's lead over the next rank vs the board's typical between-rank step), and velocity (score gain rate vs the board's peer p95). A player flagged by multiple checks, or across multiple boards, is higher confidence.": "ボードごとに 3 つの独立した統計チェックを行います：修正 Z スコア（中央値ベース。チーターが自身の基準を汚染しても影響を受けにくい）、ランク差比（次順位との差と、ボードの典型的なランク間ステップとの比較）、速度（スコア上昇率とボード内ピア p95 との比較）。複数のチェックで、または複数のボードで検出されたプレイヤーは、より確度が高くなります。",
        "Flagged {n} player(s) across {b} board(s) — based on the capture from {when}.": "{when} のキャプチャに基づき、{b} ボードで {n} 名を検出しました。",
        "Scanned {b} board(s) from the capture at {when} — nothing anomalous.": "{when} のキャプチャから {b} ボードを確認しました — 異常なし。",
        "No capture available yet to analyse.": "分析対象のキャプチャがまだありません。",
        "{n} board(s)": "{n} ボード",
        "{n} flag(s)": "検出 {n} 件",
        "View history": "履歴を表示",
        "Score outlier": "スコア外れ値",
        "Rank gap": "順位差",
        "Velocity": "速度",
    },
    "zh-CN": {
        "Possible cheaters": "可能的作弊者",
        "Checking the latest capture…": "正在检查最新采集…",
        "No suspicious activity flagged.": "未发现可疑活动。",
        "How is this computed?": "这是如何计算的？",
        "Three independent statistical checks per board: Modified Z-score (median-based, robust to cheaters polluting their own baseline), rank-gap ratio (a player's lead over the next rank vs the board's typical between-rank step), and velocity (score gain rate vs the board's peer p95). A player flagged by multiple checks, or across multiple boards, is higher confidence.": "每个榜单进行三项独立统计检查：修正 Z 分数（基于中位数，能抵抗作弊者污染自身基线）、排名差比（玩家相对下一名的领先量 vs 该榜单典型的排名间隔）和速度（分数增长速率 vs 榜单内同侪的 p95）。被多个检查命中，或在多个榜单上被命中的玩家，置信度更高。",
        "Flagged {n} player(s) across {b} board(s) — based on the capture from {when}.": "在 {b} 个榜单中标记了 {n} 位玩家 — 基于 {when} 的采集。",
        "Scanned {b} board(s) from the capture at {when} — nothing anomalous.": "扫描了 {when} 采集的 {b} 个榜单 — 无异常。",
        "No capture available yet to analyse.": "暂无可分析的采集。",
        "{n} board(s)": "{n} 个榜单",
        "{n} flag(s)": "{n} 项标记",
        "View history": "查看历史",
        "Score outlier": "分数异常",
        "Rank gap": "排名差距",
        "Velocity": "速度",
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

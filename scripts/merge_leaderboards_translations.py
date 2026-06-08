"""One-shot: merge the /leaderboards page chrome translations into each
locale JSON. Additive — existing keys are preserved, only missing ones are
added. Same pattern as scripts/merge_translations.py, kept separate so
landing-page translations and leaderboard-page translations evolve
independently.

Run: `python scripts/merge_leaderboards_translations.py`
"""
from __future__ import annotations

import json
from pathlib import Path

TRANSLATIONS: dict[str, dict[str, str]] = {

    # ============================== FRENCH ==============================
    "fr": {
        "Leaderboards": "Classements",
        "Trove leaderboards": "Classements Trove",
        "Top players per board, ranked. Hourly captures, 3-day live retention, full archive beyond.": "Meilleurs joueurs par classement, par rang. Captures horaires, 3 jours en direct, archive complète au-delà.",
        "Capture": "Capture",
        "Player history": "Historique du joueur",
        "Loading…": "Chargement…",
        "Search a player name…": "Cherchez un nom de joueur…",
        "Filter boards…": "Filtrer les classements…",
        "Board": "Classement",
        "Choose a board…": "Choisissez un classement…",
        "Pick a board to see entries": "Choisissez un classement pour voir les entrées",
        "Choose a board on the left to load its ranked entries.": "Choisissez un classement à gauche pour charger ses entrées classées.",
        "Load more": "Charger plus",
        "Captures come from an in-game scraping bot every hour. Boards reset on the same cadence Trove does (daily 11:00 UTC; weekly contests roll Mondays).": "Les captures proviennent d'un bot dans le jeu, chaque heure. Les classements se réinitialisent à la même cadence que Trove (quotidien 11:00 UTC ; concours hebdomadaires le lundi).",
        "No captures yet": "Aucune capture pour l'instant",
        "No leaderboard data has been captured yet. Check back later.": "Aucune donnée de classement n'a encore été capturée. Revenez plus tard.",
        "No boards for this capture.": "Aucun classement pour cette capture.",
        "No boards match your filter.": "Aucun classement ne correspond à votre filtre.",
        "No entries for this board.": "Aucune entrée pour ce classement.",
        "No recent appearances found for this player.": "Aucune apparition récente trouvée pour ce joueur.",
        "Failed to load": "Échec du chargement",
        "Rank": "Rang",
        "Player": "Joueur",
        "Score": "Score",
        "entries": "entrées",
    },

    # ============================== GERMAN ==============================
    "de": {
        "Leaderboards": "Bestenlisten",
        "Trove leaderboards": "Trove-Bestenlisten",
        "Top players per board, ranked. Hourly captures, 3-day live retention, full archive beyond.": "Top-Spieler pro Liste, sortiert. Stündliche Erfassung, 3 Tage live, danach vollständiges Archiv.",
        "Capture": "Erfassung",
        "Player history": "Spielerverlauf",
        "Loading…": "Lade…",
        "Search a player name…": "Spielernamen suchen…",
        "Filter boards…": "Listen filtern…",
        "Board": "Liste",
        "Choose a board…": "Wähle eine Liste…",
        "Pick a board to see entries": "Wähle eine Liste, um Einträge zu sehen",
        "Choose a board on the left to load its ranked entries.": "Wähle links eine Liste, um ihre Einträge zu laden.",
        "Load more": "Mehr laden",
        "Captures come from an in-game scraping bot every hour. Boards reset on the same cadence Trove does (daily 11:00 UTC; weekly contests roll Mondays).": "Erfassungen kommen stündlich von einem In-Game-Scraping-Bot. Listen werden im selben Rhythmus zurückgesetzt wie Trove (täglich 11:00 UTC; wöchentliche Wettbewerbe montags).",
        "No captures yet": "Noch keine Erfassungen",
        "No leaderboard data has been captured yet. Check back later.": "Es wurden noch keine Bestenlisten-Daten erfasst. Schau später wieder vorbei.",
        "No boards for this capture.": "Keine Listen für diese Erfassung.",
        "No boards match your filter.": "Keine Listen entsprechen dem Filter.",
        "No entries for this board.": "Keine Einträge für diese Liste.",
        "No recent appearances found for this player.": "Keine aktuellen Auftritte für diesen Spieler gefunden.",
        "Failed to load": "Laden fehlgeschlagen",
        "Rank": "Rang",
        "Player": "Spieler",
        "Score": "Punkte",
        "entries": "Einträge",
    },

    # ============================ PORTUGUESE ============================
    "pt-PT": {
        "Leaderboards": "Classificações",
        "Trove leaderboards": "Classificações Trove",
        "Top players per board, ranked. Hourly captures, 3-day live retention, full archive beyond.": "Melhores jogadores por classificação, ordenados. Capturas horárias, 3 dias em direto, arquivo completo para além disso.",
        "Capture": "Captura",
        "Player history": "Histórico do jogador",
        "Loading…": "A carregar…",
        "Search a player name…": "Procurar um nome de jogador…",
        "Filter boards…": "Filtrar classificações…",
        "Board": "Classificação",
        "Choose a board…": "Escolha uma classificação…",
        "Pick a board to see entries": "Escolha uma classificação para ver as entradas",
        "Choose a board on the left to load its ranked entries.": "Escolha uma classificação à esquerda para carregar as suas entradas.",
        "Load more": "Carregar mais",
        "Captures come from an in-game scraping bot every hour. Boards reset on the same cadence Trove does (daily 11:00 UTC; weekly contests roll Mondays).": "As capturas vêm de um bot dentro do jogo, a cada hora. As classificações reiniciam à mesma cadência do Trove (diariamente às 11:00 UTC; concursos semanais rolam às segundas-feiras).",
        "No captures yet": "Ainda sem capturas",
        "No leaderboard data has been captured yet. Check back later.": "Ainda não foram capturados dados de classificações. Volte mais tarde.",
        "No boards for this capture.": "Sem classificações para esta captura.",
        "No boards match your filter.": "Nenhuma classificação corresponde ao filtro.",
        "No entries for this board.": "Sem entradas para esta classificação.",
        "No recent appearances found for this player.": "Não foram encontradas aparições recentes deste jogador.",
        "Failed to load": "Falha ao carregar",
        "Rank": "Posição",
        "Player": "Jogador",
        "Score": "Pontuação",
        "entries": "entradas",
    },

    # ============================== RUSSIAN ==============================
    "ru": {
        "Leaderboards": "Таблицы лидеров",
        "Trove leaderboards": "Таблицы лидеров Trove",
        "Top players per board, ranked. Hourly captures, 3-day live retention, full archive beyond.": "Лучшие игроки по каждой таблице, по рангу. Ежечасные снимки, 3 дня в живом доступе, далее — полный архив.",
        "Capture": "Снимок",
        "Player history": "История игрока",
        "Loading…": "Загрузка…",
        "Search a player name…": "Найдите имя игрока…",
        "Filter boards…": "Фильтр таблиц…",
        "Board": "Таблица",
        "Choose a board…": "Выберите таблицу…",
        "Pick a board to see entries": "Выберите таблицу, чтобы увидеть записи",
        "Choose a board on the left to load its ranked entries.": "Выберите таблицу слева, чтобы загрузить её ранжированные записи.",
        "Load more": "Загрузить ещё",
        "Captures come from an in-game scraping bot every hour. Boards reset on the same cadence Trove does (daily 11:00 UTC; weekly contests roll Mondays).": "Снимки делает игровой скрейпинг-бот каждый час. Таблицы сбрасываются в том же ритме, что и в Trove (ежедневно в 11:00 UTC; еженедельные конкурсы — по понедельникам).",
        "No captures yet": "Снимков пока нет",
        "No leaderboard data has been captured yet. Check back later.": "Данные таблиц лидеров пока не собраны. Загляните позже.",
        "No boards for this capture.": "Для этого снимка нет таблиц.",
        "No boards match your filter.": "Ни одна таблица не подходит под фильтр.",
        "No entries for this board.": "В этой таблице нет записей.",
        "No recent appearances found for this player.": "Недавних появлений этого игрока не найдено.",
        "Failed to load": "Не удалось загрузить",
        "Rank": "Ранг",
        "Player": "Игрок",
        "Score": "Очки",
        "entries": "записей",
    },

    # ============================== JAPANESE ==============================
    "ja": {
        "Leaderboards": "ランキング",
        "Trove leaderboards": "Trove ランキング",
        "Top players per board, ranked. Hourly captures, 3-day live retention, full archive beyond.": "ボードごとの上位プレイヤー、順位付き。毎時キャプチャ、3 日間のライブ保持、それ以降は完全アーカイブ。",
        "Capture": "キャプチャ",
        "Player history": "プレイヤーの履歴",
        "Loading…": "読み込み中…",
        "Search a player name…": "プレイヤー名を検索…",
        "Filter boards…": "ボードを絞り込み…",
        "Board": "ボード",
        "Choose a board…": "ボードを選択…",
        "Pick a board to see entries": "ボードを選択してエントリを表示",
        "Choose a board on the left to load its ranked entries.": "左側のボードを選択して、順位付きエントリを読み込みます。",
        "Load more": "もっと読み込む",
        "Captures come from an in-game scraping bot every hour. Boards reset on the same cadence Trove does (daily 11:00 UTC; weekly contests roll Mondays).": "キャプチャはゲーム内のスクレイピングボットから毎時取得されます。ボードは Trove と同じサイクルでリセットされます（毎日 11:00 UTC、週次コンテストは月曜日）。",
        "No captures yet": "キャプチャはまだありません",
        "No leaderboard data has been captured yet. Check back later.": "ランキングデータはまだ取得されていません。後でもう一度ご確認ください。",
        "No boards for this capture.": "このキャプチャにはボードがありません。",
        "No boards match your filter.": "フィルタに一致するボードはありません。",
        "No entries for this board.": "このボードにはエントリがありません。",
        "No recent appearances found for this player.": "このプレイヤーの最近の出現は見つかりませんでした。",
        "Failed to load": "読み込み失敗",
        "Rank": "順位",
        "Player": "プレイヤー",
        "Score": "スコア",
        "entries": "エントリ",
    },

    # ========================== SIMPLIFIED CHINESE =========================
    "zh-CN": {
        "Leaderboards": "排行榜",
        "Trove leaderboards": "Trove 排行榜",
        "Top players per board, ranked. Hourly captures, 3-day live retention, full archive beyond.": "每个榜单的顶尖玩家，按排名展示。每小时采集，3 天热数据保留，更早的进入完整归档。",
        "Capture": "采集",
        "Player history": "玩家历史",
        "Loading…": "加载中…",
        "Search a player name…": "搜索玩家名…",
        "Filter boards…": "筛选榜单…",
        "Board": "榜单",
        "Choose a board…": "选择一个榜单…",
        "Pick a board to see entries": "选择一个榜单来查看记录",
        "Choose a board on the left to load its ranked entries.": "在左侧选择一个榜单，加载其排名记录。",
        "Load more": "加载更多",
        "Captures come from an in-game scraping bot every hour. Boards reset on the same cadence Trove does (daily 11:00 UTC; weekly contests roll Mondays).": "采集由游戏内抓取机器人每小时执行。榜单按 Trove 自身的节奏重置（每日 11:00 UTC；每周比赛在周一轮换）。",
        "No captures yet": "暂无采集",
        "No leaderboard data has been captured yet. Check back later.": "尚未采集到排行榜数据。请稍后再来查看。",
        "No boards for this capture.": "此次采集没有榜单。",
        "No boards match your filter.": "没有匹配筛选条件的榜单。",
        "No entries for this board.": "此榜单暂无记录。",
        "No recent appearances found for this player.": "未找到此玩家的近期上榜记录。",
        "Failed to load": "加载失败",
        "Rank": "排名",
        "Player": "玩家",
        "Score": "分数",
        "entries": "条记录",
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
        print(f"  {lang}: added {added:>3} keys (total now {len(existing)})")


if __name__ == "__main__":
    merge()

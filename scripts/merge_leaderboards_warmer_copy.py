"""One-shot: add the "Crunching latest data" placeholder copy + the
new /support page strings to each locale JSON. Additive — existing
keys are preserved.

Run: `python scripts/merge_leaderboards_warmer_copy.py`
"""
from __future__ import annotations

import json
from pathlib import Path

# Each key is the EXACT English source string. Translations omitted
# for a (key, locale) pair fall through to English at render time.
TRANSLATIONS: dict[str, dict[str, str]] = {
    # Leaderboards loading copy
    "Crunching the latest capture — first paint can take a moment while we warm the caches.": {
        "fr": "Calcul de la dernière capture — le premier affichage peut prendre un moment pendant que nous chauffons les caches.",
        "de": "Letzte Aufnahme wird verarbeitet — die erste Anzeige kann einen Moment dauern, während die Caches warmlaufen.",
        "pt-PT": "A processar a captura mais recente — a primeira renderização pode demorar enquanto aquecemos as caches.",
        "ru": "Обрабатываем последнюю запись — первый показ может занять момент, пока прогреваются кэши.",
        "ja": "最新キャプチャを処理中 — キャッシュをウォームアップするため、最初の表示に少々時間がかかることがあります。",
        "zh-CN": "正在处理最新捕获——首次显示可能稍等片刻，缓存正在预热。",
    },

    # /support page strings
    "Support": {
        "fr": "Soutien", "de": "Unterstützen", "pt-PT": "Apoiar",
        "ru": "Поддержать", "ja": "支援", "zh-CN": "支持",
    },
    "Support the project": {
        "fr": "Soutenir le projet", "de": "Projekt unterstützen",
        "pt-PT": "Apoiar o projeto", "ru": "Поддержать проект",
        "ja": "プロジェクトを支援する", "zh-CN": "支持该项目",
    },
    "Better Trove Tools is free, open source, and built in spare time. Every page you've used today — the leaderboards, the updates browser, the commands index, the desktop app — runs on a handful of small servers and a few domains I pay for monthly. If any of it saved you time, a tip helps keep the lights on.": {
        "fr": "Better Trove Tools est gratuit, open source, et construit pendant mon temps libre. Chaque page que vous avez utilisée aujourd'hui — les classements, l'explorateur de mises à jour, l'index des commandes, l'app de bureau — tourne sur quelques petits serveurs et des domaines que je paie chaque mois. Si quoi que ce soit vous a fait gagner du temps, un pourboire aide à garder les lumières allumées.",
        "de": "Better Trove Tools ist kostenlos, Open Source und in der Freizeit gebaut. Jede Seite, die du heute genutzt hast — die Bestenlisten, der Updates-Browser, das Befehlsverzeichnis, die Desktop-App — läuft auf wenigen kleinen Servern und Domains, die ich monatlich bezahle. Wenn dir etwas davon Zeit gespart hat, hilft ein Trinkgeld, das Licht anzulassen.",
        "pt-PT": "Better Trove Tools é grátis, open source e feito em tempo livre. Cada página que usaste hoje — as classificações, o explorador de atualizações, o índice de comandos, a app desktop — corre num punhado de pequenos servidores e domínios que pago todos os meses. Se algo te poupou tempo, uma gorjeta ajuda a manter as luzes acesas.",
        "ru": "Better Trove Tools — бесплатный, с открытым исходным кодом и собран в свободное время. Каждая страница, которую вы сегодня использовали — таблицы лидеров, браузер обновлений, индекс команд, десктоп-приложение — работает на нескольких небольших серверах и доменах, за которые я плачу ежемесячно. Если что-то из этого сэкономило вам время, чаевые помогают держать свет включённым.",
        "ja": "Better Trove Tools は無料・オープンソースで、空き時間に作られています。今日使用した各ページ — リーダーボード、アップデートブラウザ、コマンド一覧、デスクトップアプリ — は、私が月額で支払っている少数のサーバーとドメインの上で動いています。何かしらでお時間を節約できたなら、チップが運営を支えてくれます。",
        "zh-CN": "Better Trove Tools 是免费、开源、在闲暇时间打造的。你今天用过的每一页 — 排行榜、更新浏览器、命令索引、桌面应用 — 都跑在我每月付费的几台小服务器和几个域名上。如果其中任何一样为你节省了时间，打赏可以让灯继续亮着。",
    },
    "One-off or recurring — your usual PayPal balance / card.": {
        "fr": "Ponctuel ou récurrent — votre solde PayPal habituel ou carte.",
        "de": "Einmalig oder wiederkehrend — dein gewohntes PayPal-Guthaben oder Karte.",
        "pt-PT": "Pontual ou recorrente — o teu saldo PayPal habitual ou cartão.",
        "ru": "Разово или регулярно — обычный баланс PayPal или карта.",
        "ja": "一回限りでも継続でも — 普段の PayPal 残高またはカードで。",
        "zh-CN": "一次性或定期 — 用你常用的 PayPal 余额或银行卡。",
    },
    "The price of a coffee. Quick, no account required.": {
        "fr": "Le prix d'un café. Rapide, sans compte requis.",
        "de": "Der Preis eines Kaffees. Schnell, kein Konto erforderlich.",
        "pt-PT": "O preço de um café. Rápido, não precisa de conta.",
        "ru": "Цена чашки кофе. Быстро, без регистрации.",
        "ja": "コーヒー一杯のお値段。アカウント不要、すぐに。",
        "zh-CN": "一杯咖啡的价钱。快捷无需账户。",
    },
    "Same idea as Ko-fi — pick whichever you prefer.": {
        "fr": "Même idée que Ko-fi — choisissez celui que vous préférez.",
        "de": "Gleiche Idee wie Ko-fi — wähle, was du bevorzugst.",
        "pt-PT": "Mesma ideia do Ko-fi — escolhe o que preferires.",
        "ru": "То же, что и Ko-fi — выберите, что удобнее.",
        "ja": "Ko-fi と同じです — お好みでどうぞ。",
        "zh-CN": "和 Ko-fi 一样 — 选你喜欢的即可。",
    },
    "Where the money goes": {
        "fr": "Où va l'argent",
        "de": "Wohin das Geld geht",
        "pt-PT": "Para onde vai o dinheiro",
        "ru": "Куда идут деньги",
        "ja": "お金の使い道",
        "zh-CN": "钱花在哪里",
    },
    "API hosting": {
        "fr": "Hébergement de l'API", "de": "API-Hosting",
        "pt-PT": "Alojamento da API", "ru": "Хостинг API",
        "ja": "API ホスティング", "zh-CN": "API 托管",
    },
    "Domains": {
        "fr": "Domaines", "de": "Domains",
        "pt-PT": "Domínios", "ru": "Домены",
        "ja": "ドメイン", "zh-CN": "域名",
    },
    "Cloudflare + edge": {
        "fr": "Cloudflare + bord", "de": "Cloudflare + Edge",
        "pt-PT": "Cloudflare + edge", "ru": "Cloudflare + edge",
        "ja": "Cloudflare + エッジ", "zh-CN": "Cloudflare + 边缘",
    },
    "Time and coffee": {
        "fr": "Du temps et du café", "de": "Zeit und Kaffee",
        "pt-PT": "Tempo e café", "ru": "Время и кофе",
        "ja": "時間とコーヒー", "zh-CN": "时间和咖啡",
    },
    "Can't tip? That's fine — here's free help": {
        "fr": "Pas de pourboire ? Pas grave — voici comment aider gratuitement",
        "de": "Kein Trinkgeld? Auch okay — so kannst du gratis helfen",
        "pt-PT": "Sem dar gorjeta? Tudo bem — eis como ajudar de graça",
        "ru": "Не можете оставить чаевые? Не страшно — вот как помочь бесплатно",
        "ja": "チップが難しい？大丈夫 — 無料でできる支援はこちら",
        "zh-CN": "不方便打赏？没关系 — 这些方式也能帮上忙",
    },
    "Star the repo": {
        "fr": "Mettre une étoile au dépôt",
        "de": "Repo mit Stern markieren",
        "pt-PT": "Dar estrela ao repositório",
        "ru": "Поставить звезду репозиторию",
        "ja": "リポジトリにスターを付ける",
        "zh-CN": "给仓库点星",
    },
    "Helps with discoverability and tells me the project's worth keeping warm.": {
        "fr": "Aide à la découverte et me confirme que le projet vaut la peine d'être maintenu.",
        "de": "Hilft bei der Auffindbarkeit und sagt mir, dass das Projekt es wert ist, gepflegt zu werden.",
        "pt-PT": "Ajuda na descoberta e diz-me que o projeto vale a pena manter.",
        "ru": "Помогает с обнаружением и говорит мне, что проект стоит поддерживать.",
        "ja": "発見性向上に役立ち、プロジェクトを続ける価値があると伝わります。",
        "zh-CN": "提升曝光，让我知道项目值得继续维护。",
    },
    "Hop on Discord": {
        "fr": "Rejoindre le Discord",
        "de": "Auf Discord vorbeischauen",
        "pt-PT": "Vem ao Discord",
        "ru": "Заглянуть в Discord",
        "ja": "Discord に参加",
        "zh-CN": "加入 Discord",
    },
    "Report bugs, request features, share builds. The fastest channel to me.": {
        "fr": "Signaler des bugs, demander des fonctionnalités, partager des builds. Le canal le plus rapide pour me joindre.",
        "de": "Bugs melden, Features wünschen, Builds teilen. Der schnellste Kanal zu mir.",
        "pt-PT": "Reportar bugs, pedir funcionalidades, partilhar builds. O canal mais rápido para me chegar.",
        "ru": "Сообщайте об ошибках, предлагайте функции, делитесь сборками. Самый быстрый канал ко мне.",
        "ja": "バグ報告、機能リクエスト、ビルド共有。最速の連絡手段です。",
        "zh-CN": "报告 bug、提建议、分享构筑。联系我最快的渠道。",
    },
    "Tell a Trove friend": {
        "fr": "En parler à un ami Trove",
        "de": "Einem Trove-Freund erzählen",
        "pt-PT": "Conta a um amigo Trove",
        "ru": "Расскажите другу по Trove",
        "ja": "Trove 友達に教える",
        "zh-CN": "告诉一个 Trove 朋友",
    },
    "If a tool here saved you ten minutes once, it'll do the same for someone else.": {
        "fr": "Si un outil ici vous a fait gagner dix minutes une fois, il fera pareil pour quelqu'un d'autre.",
        "de": "Wenn ein Tool hier dir einmal zehn Minuten gespart hat, wird es das auch jemand anderem tun.",
        "pt-PT": "Se uma ferramenta aqui te poupou dez minutos uma vez, fará o mesmo a outra pessoa.",
        "ru": "Если какой-то инструмент здесь однажды сэкономил вам десять минут, он сделает то же для кого-то ещё.",
        "ja": "ここのツールが一度でも10分を節約してくれたなら、誰かにも同じ価値があります。",
        "zh-CN": "如果这里的某个工具曾为你节省过十分钟，对别人也同样如此。",
    },
    "Either way — thanks for using the tools. ❤": {
        "fr": "Quoi qu'il en soit — merci d'utiliser les outils. ❤",
        "de": "So oder so — danke, dass du die Tools nutzt. ❤",
        "pt-PT": "De qualquer forma — obrigado por usares as ferramentas. ❤",
        "ru": "В любом случае — спасибо, что пользуетесь инструментами. ❤",
        "ja": "どちらにせよ — ツールを使ってくれてありがとう。❤",
        "zh-CN": "无论如何 — 感谢使用这些工具。❤",
    },

    # Bullet bodies — kept English-only by default would be a shame; here
    # are the localised versions
    "The Kiwi API runs on a VPS — leaderboards, updates archive (the deduped blob store + Mongo), market data, the rotation history. Most of your monthly cost.": {
        "fr": "L'API Kiwi tourne sur un VPS — classements, archive des mises à jour (le stockage de blobs dédupliqué + Mongo), données du marché, historique des rotations. La majeure partie du coût mensuel.",
        "de": "Die Kiwi API läuft auf einem VPS — Bestenlisten, Updates-Archiv (deduplizierter Blob-Speicher + Mongo), Marktdaten, Rotationsverlauf. Der größte monatliche Posten.",
        "pt-PT": "A Kiwi API corre num VPS — classificações, arquivo de atualizações (armazenamento de blobs deduplicado + Mongo), dados do mercado, histórico de rotações. Maior parte do custo mensal.",
        "ru": "Kiwi API крутится на VPS — таблицы лидеров, архив обновлений (дедуплицированное хранилище + Mongo), данные рынка, история ротаций. Большая часть ежемесячных расходов.",
        "ja": "Kiwi API は VPS 上で稼働 — リーダーボード、アップデートアーカイブ（重複排除ブロブストア + Mongo）、マーケットデータ、ローテーション履歴。月額コストの大部分を占めます。",
        "zh-CN": "Kiwi API 跑在 VPS 上 — 排行榜、更新存档（去重 blob 存储 + Mongo）、市场数据、轮换历史。占月度成本的大头。",
    },
    "aallyn.net plus the subdomains for the site, app, dev portal, and API. Renewed annually.": {
        "fr": "aallyn.net plus les sous-domaines pour le site, l'app, le portail dev et l'API. Renouvelés chaque année.",
        "de": "aallyn.net plus die Subdomains für Website, App, Dev-Portal und API. Jährlich erneuert.",
        "pt-PT": "aallyn.net mais os subdomínios para o site, app, portal dev e API. Renovados anualmente.",
        "ru": "aallyn.net плюс поддомены для сайта, приложения, дев-портала и API. Продлеваются ежегодно.",
        "ja": "aallyn.net とサイト・アプリ・開発者ポータル・API のサブドメイン。毎年更新。",
        "zh-CN": "aallyn.net 加上网站、应用、开发者门户和 API 的子域名。每年续费。",
    },
    "CDN, WAF, and image hosting in front of everything. Free tier mostly, paid where it matters.": {
        "fr": "CDN, WAF et hébergement d'images devant tout. Surtout le palier gratuit, payant là où ça compte.",
        "de": "CDN, WAF und Bild-Hosting vor allem. Größtenteils Free Tier, bezahlt wo es wichtig ist.",
        "pt-PT": "CDN, WAF e alojamento de imagens à frente de tudo. Maioritariamente plano gratuito, pago onde é preciso.",
        "ru": "CDN, WAF и хостинг изображений перед всем. В основном бесплатный план, платно — где это важно.",
        "ja": "すべての前段に CDN、WAF、画像ホスティング。多くは無料枠、必要な部分のみ有料。",
        "zh-CN": "CDN、WAF 和图片托管放在最前面。大部分是免费层，关键的地方花钱。",
    },
    "Everything else funds the dev hours. No salary out of this — it's a hobby project. But beans are not free.": {
        "fr": "Le reste finance les heures de dev. Pas de salaire ici — c'est un projet de loisir. Mais les grains de café ne sont pas gratuits.",
        "de": "Der Rest finanziert Dev-Stunden. Kein Gehalt — das ist ein Hobby-Projekt. Aber Bohnen sind nicht gratis.",
        "pt-PT": "O resto financia as horas de desenvolvimento. Não há salário — é um projeto de hobby. Mas os grãos não são gratuitos.",
        "ru": "Остальное идёт на часы разработки. Зарплаты с этого нет — это хобби. Но зёрна не бесплатны.",
        "ja": "残りは開発時間に充てます。給料は出ません — 趣味のプロジェクトです。でも豆はタダではありません。",
        "zh-CN": "其余支撑开发时间。没有工资 — 这是兴趣项目。但咖啡豆可不免费。",
    },
}


def merge() -> None:
    locales_dir = Path("site/static/locales")
    languages: set[str] = set()
    for translations in TRANSLATIONS.values():
        languages.update(translations.keys())

    for lang in sorted(languages):
        target = locales_dir / f"{lang}.json"
        if not target.exists():
            print(f"  skip {lang} (no file)")
            continue
        with target.open("r", encoding="utf-8") as f:
            existing = json.load(f)
        added = 0
        for english, per_lang in TRANSLATIONS.items():
            if english in existing:
                continue
            value = per_lang.get(lang)
            if value is None:
                continue
            existing[english] = value
            added += 1
        if not added:
            print(f"  {lang}: already up to date")
            continue
        with target.open("w", encoding="utf-8") as f:
            json.dump(existing, f, ensure_ascii=False, indent=2, sort_keys=True)
            f.write("\n")
        print(f"  {lang}: added {added} key(s) (total now {len(existing)})")


if __name__ == "__main__":
    merge()

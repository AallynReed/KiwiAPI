"""One-shot: add translation keys for the new /updates page (game updates
file explorer + change browser + per-file compare). Additive — existing
keys are preserved.

Run: `python scripts/merge_updates_translations.py`
"""
from __future__ import annotations

import json
from pathlib import Path

# Each key is the EXACT English source. The script writes it into each
# locale JSON file as `{source: translated}`. Translations omitted for
# a (key, locale) pair fall through to English at render time (i18n.js
# returns the source when the dict lookup misses).
TRANSLATIONS: dict[str, dict[str, str]] = {
    # Nav / page chrome
    "Updates": {
        "fr": "Mises à jour", "de": "Updates", "pt-PT": "Atualizações",
        "ru": "Обновления", "ja": "アップデート", "zh-CN": "更新",
    },
    "Trove game updates": {
        "fr": "Mises à jour de Trove", "de": "Trove-Spielupdates",
        "pt-PT": "Atualizações do jogo Trove", "ru": "Игровые обновления Trove",
        "ja": "Troveのゲーム更新", "zh-CN": "Trove 游戏更新",
    },
    "Every captured Trove patch on both servers, file by file. Drill into the tree, see what each version changed, and diff any two versions of any file.": {
        "fr": "Chaque patch Trove capturé sur les deux serveurs, fichier par fichier. Parcourez l'arborescence, voyez ce que chaque version a changé et comparez deux versions de n'importe quel fichier.",
        "de": "Jeder erfasste Trove-Patch auf beiden Servern, Datei für Datei. Durchsuche den Baum, sieh, was jede Version geändert hat, und vergleiche zwei beliebige Versionen einer Datei.",
        "pt-PT": "Cada patch do Trove capturado em ambos os servidores, ficheiro a ficheiro. Explore a árvore, veja o que cada versão alterou e compare duas versões de qualquer ficheiro.",
        "ru": "Каждый патч Trove, захваченный с обоих серверов, файл за файлом. Изучайте дерево, смотрите, что изменила каждая версия, и сравнивайте любые две версии любого файла.",
        "ja": "両サーバーで取得したTroveパッチを一つ一つファイル単位で。ツリーを掘り下げ、各バージョンで何が変わったかを確認し、任意の2つのバージョンを比較できます。",
        "zh-CN": "在两个服务器上每一次捕获的 Trove 补丁，按文件呈现。深入文件树，查看每个版本的变化，对比任意两个版本。",
    },
    "Captures mirror Trion's CDN into a deduped blob store. Each pointer change triggers a fetch of just the files whose opaque manifest hash moved — same delta the launcher would download.": {
        "fr": "Les captures reflètent le CDN de Trion dans un stockage dédupliqué. Chaque changement de pointeur déclenche le téléchargement uniquement des fichiers dont le hash de manifeste a évolué — le même delta que le lanceur télécharge.",
        "de": "Die Aufnahmen spiegeln Trions CDN in einen deduplizierten Blob-Speicher. Jede Pointer-Änderung löst den Abruf nur jener Dateien aus, deren Manifest-Hash sich geändert hat — derselbe Delta, den der Launcher herunterladen würde.",
        "pt-PT": "As capturas espelham o CDN da Trion num armazenamento de blobs deduplicado. Cada mudança de ponteiro descarrega apenas os ficheiros cujo hash de manifesto mudou — o mesmo delta que o launcher transferiria.",
        "ru": "Захваты зеркалируют CDN Trion в дедуплицированное хранилище блобов. Каждое изменение указателя запускает загрузку только тех файлов, у которых поменялся хэш манифеста — та же дельта, что и у лаунчера.",
        "ja": "キャプチャは、Trion の CDN を重複排除済みブロブストアにミラーリングします。ポインタの変化があれば、不透明なマニフェストハッシュが変わったファイルだけを取得します ― ランチャーがダウンロードするのと同じデルタです。",
        "zh-CN": "捕获将 Trion 的 CDN 镜像到去重的 Blob 存储。每次指针变化只会下载清单哈希变动的文件——与启动器下载的增量相同。",
    },
    "Recent versions": {
        "fr": "Versions récentes", "de": "Aktuelle Versionen",
        "pt-PT": "Versões recentes", "ru": "Последние версии",
        "ja": "最近のバージョン", "zh-CN": "最近版本",
    },
    "File explorer": {
        "fr": "Explorateur de fichiers", "de": "Datei-Explorer",
        "pt-PT": "Explorador de ficheiros", "ru": "Проводник файлов",
        "ja": "ファイルエクスプローラー", "zh-CN": "文件浏览器",
    },
    "This version's changes": {
        "fr": "Modifications de cette version", "de": "Änderungen dieser Version",
        "pt-PT": "Alterações desta versão", "ru": "Изменения этой версии",
        "ja": "このバージョンの変更", "zh-CN": "本版本的更改",
    },
    "Compare versions": {
        "fr": "Comparer les versions", "de": "Versionen vergleichen",
        "pt-PT": "Comparar versões", "ru": "Сравнить версии",
        "ja": "バージョンを比較", "zh-CN": "对比版本",
    },
    "Pick a file from the tree to see its full version history and diff any two captures.": {
        "fr": "Sélectionnez un fichier dans l'arborescence pour voir tout son historique et comparer deux captures.",
        "de": "Wähle eine Datei aus dem Baum, um ihren vollständigen Verlauf zu sehen und zwei beliebige Aufnahmen zu vergleichen.",
        "pt-PT": "Selecione um ficheiro da árvore para ver o histórico completo de versões e comparar duas capturas.",
        "ru": "Выберите файл в дереве, чтобы увидеть всю историю версий и сравнить любые две его записи.",
        "ja": "ツリーからファイルを選択すると、全バージョン履歴を確認し、任意の2件を比較できます。",
        "zh-CN": "在树中选择一个文件，即可查看完整版本历史，并对比任意两个捕获。",
    },
    "Version history": {
        "fr": "Historique des versions", "de": "Versionsverlauf",
        "pt-PT": "Histórico de versões", "ru": "История версий",
        "ja": "バージョン履歴", "zh-CN": "版本历史",
    },
    "Pick a version above to see its change-list.": {
        "fr": "Choisissez une version ci-dessus pour voir sa liste de modifications.",
        "de": "Wähle oben eine Version, um ihre Änderungsliste zu sehen.",
        "pt-PT": "Escolha uma versão acima para ver a respetiva lista de alterações.",
        "ru": "Выберите версию выше, чтобы увидеть её список изменений.",
        "ja": "上記からバージョンを選択して変更一覧を表示します。",
        "zh-CN": "选择上方的版本以查看其更改列表。",
    },
    "Select a version chip above to populate this list.": {
        "fr": "Cliquez sur une vignette de version ci-dessus pour remplir cette liste.",
        "de": "Klicke oben auf einen Versions-Chip, um diese Liste zu füllen.",
        "pt-PT": "Clique numa versão acima para preencher esta lista.",
        "ru": "Нажмите на чип версии выше, чтобы заполнить этот список.",
        "ja": "上のバージョンチップをクリックして一覧を表示します。",
        "zh-CN": "点击上方的版本卡片以填充此列表。",
    },
    "Fill in a path, pick two versions, then press Compare. Text files are diffed inline; binaries show their size and content hash on each side.": {
        "fr": "Saisissez un chemin, choisissez deux versions, puis cliquez sur Comparer. Les fichiers texte sont comparés en ligne ; les binaires affichent leur taille et hachage de contenu de chaque côté.",
        "de": "Pfad eintragen, zwei Versionen auswählen und auf Vergleichen klicken. Textdateien werden inline differenziert; Binärdateien zeigen Größe und Inhalts-Hash beider Seiten.",
        "pt-PT": "Indique um caminho, escolha duas versões e prima Comparar. Os ficheiros de texto são comparados inline; binários mostram tamanho e hash de cada lado.",
        "ru": "Укажите путь, выберите две версии и нажмите «Сравнить». Текстовые файлы сравниваются построчно; бинарные показывают размер и хэш содержимого с каждой стороны.",
        "ja": "パスを入力し、2つのバージョンを選択して「比較」を押します。テキストファイルはインラインで差分表示され、バイナリは各側のサイズとコンテンツハッシュを表示します。",
        "zh-CN": "填入路径，选择两个版本，然后点击对比。文本文件会内联展示差异；二进制文件会显示两侧的大小与内容哈希。",
    },
    "Browse the tree…": {
        "fr": "Parcourir l'arborescence…", "de": "Baum durchsuchen…",
        "pt-PT": "Explorar a árvore…", "ru": "Просмотреть дерево…",
        "ja": "ツリーを参照…", "zh-CN": "浏览文件树…",
    },
    "Filter children…": {
        "fr": "Filtrer les enfants…", "de": "Unterelemente filtern…",
        "pt-PT": "Filtrar elementos…", "ru": "Фильтр потомков…",
        "ja": "子要素を絞り込む…", "zh-CN": "筛选子项…",
    },
    "Logical path (e.g. prefabs/items/sword.binfab)": {
        "fr": "Chemin logique (ex. prefabs/items/sword.binfab)",
        "de": "Logischer Pfad (z. B. prefabs/items/sword.binfab)",
        "pt-PT": "Caminho lógico (ex.: prefabs/items/sword.binfab)",
        "ru": "Логический путь (например, prefabs/items/sword.binfab)",
        "ja": "論理パス (例: prefabs/items/sword.binfab)",
        "zh-CN": "逻辑路径（如 prefabs/items/sword.binfab）",
    },

    # Filter chips + simple labels
    "All": {"fr": "Tout", "de": "Alle", "pt-PT": "Tudo", "ru": "Все", "ja": "すべて", "zh-CN": "全部"},
    "Added": {"fr": "Ajouté", "de": "Hinzugefügt", "pt-PT": "Adicionado", "ru": "Добавлено", "ja": "追加", "zh-CN": "新增"},
    "Modified": {"fr": "Modifié", "de": "Geändert", "pt-PT": "Modificado", "ru": "Изменено", "ja": "変更", "zh-CN": "修改"},
    "Removed": {"fr": "Supprimé", "de": "Entfernt", "pt-PT": "Removido", "ru": "Удалено", "ja": "削除", "zh-CN": "删除"},
    "added": {"fr": "ajouté", "de": "hinzugefügt", "pt-PT": "adicionado", "ru": "добавлен", "ja": "追加", "zh-CN": "新增"},
    "modified": {"fr": "modifié", "de": "geändert", "pt-PT": "modificado", "ru": "изменён", "ja": "変更", "zh-CN": "修改"},
    "removed": {"fr": "supprimé", "de": "entfernt", "pt-PT": "removido", "ru": "удалён", "ja": "削除", "zh-CN": "删除"},

    "Path": {"fr": "Chemin", "de": "Pfad", "pt-PT": "Caminho", "ru": "Путь", "ja": "パス", "zh-CN": "路径"},
    "From": {"fr": "De", "de": "Von", "pt-PT": "De", "ru": "От", "ja": "から", "zh-CN": "从"},
    "To": {"fr": "À", "de": "Bis", "pt-PT": "Até", "ru": "До", "ja": "まで", "zh-CN": "到"},
    "Compare": {"fr": "Comparer", "de": "Vergleichen", "pt-PT": "Comparar", "ru": "Сравнить", "ja": "比較", "zh-CN": "对比"},
    "Latest": {"fr": "Dernière", "de": "Neueste", "pt-PT": "Mais recente", "ru": "Последняя", "ja": "最新", "zh-CN": "最新"},
    "Load more": {"fr": "Charger plus", "de": "Mehr laden", "pt-PT": "Carregar mais", "ru": "Загрузить ещё", "ja": "もっと読み込む", "zh-CN": "加载更多"},

    # JS dynamic strings
    "files": {"fr": "fichiers", "de": "Dateien", "pt-PT": "ficheiros", "ru": "файлов", "ja": "ファイル", "zh-CN": "个文件"},
    "captures": {"fr": "captures", "de": "Aufnahmen", "pt-PT": "capturas", "ru": "снимков", "ja": "キャプチャ", "zh-CN": "次捕获"},
    "root": {"fr": "racine", "de": "Wurzel", "pt-PT": "raiz", "ru": "корень", "ja": "ルート", "zh-CN": "根目录"},
    "Nothing here.": {"fr": "Rien ici.", "de": "Nichts hier.", "pt-PT": "Nada aqui.", "ru": "Здесь пусто.", "ja": "何もありません。", "zh-CN": "这里没有内容。"},
    "absent": {"fr": "absent", "de": "fehlt", "pt-PT": "ausente", "ru": "отсутствует", "ja": "なし", "zh-CN": "缺失"},
    "Comparing…": {"fr": "Comparaison…", "de": "Vergleicht…", "pt-PT": "A comparar…", "ru": "Сравниваем…", "ja": "比較中…", "zh-CN": "正在比较…"},
    "No changes match this filter.": {
        "fr": "Aucune modification ne correspond à ce filtre.",
        "de": "Keine Änderungen entsprechen diesem Filter.",
        "pt-PT": "Nenhuma alteração corresponde a este filtro.",
        "ru": "Нет изменений, соответствующих этому фильтру.",
        "ja": "このフィルタに一致する変更はありません。",
        "zh-CN": "没有符合此筛选条件的更改。",
    },
    "No captured versions yet.": {
        "fr": "Aucune version capturée pour le moment.",
        "de": "Noch keine erfasste Version.",
        "pt-PT": "Ainda não há versões capturadas.",
        "ru": "Версий ещё не захвачено.",
        "ja": "まだキャプチャされたバージョンはありません。",
        "zh-CN": "尚未捕获任何版本。",
    },
    "No changes between these two versions.": {
        "fr": "Aucune différence entre ces deux versions.",
        "de": "Keine Unterschiede zwischen diesen beiden Versionen.",
        "pt-PT": "Sem diferenças entre estas duas versões.",
        "ru": "Между этими двумя версиями нет различий.",
        "ja": "これら2つのバージョン間に差異はありません。",
        "zh-CN": "两个版本之间没有差异。",
    },
    "Binary file — inline diff skipped.": {
        "fr": "Fichier binaire — diff inline ignoré.",
        "de": "Binärdatei — Inline-Diff übersprungen.",
        "pt-PT": "Ficheiro binário — diff inline omitido.",
        "ru": "Двоичный файл — встроенный diff пропущен.",
        "ja": "バイナリファイル — インライン差分はスキップされました。",
        "zh-CN": "二进制文件——已跳过内联对比。",
    },
    "Enter a path first.": {
        "fr": "Saisissez d'abord un chemin.",
        "de": "Bitte zuerst einen Pfad eintragen.",
        "pt-PT": "Indique primeiro um caminho.",
        "ru": "Сначала укажите путь.",
        "ja": "先にパスを入力してください。",
        "zh-CN": "请先填写路径。",
    },
    "Pick two versions.": {
        "fr": "Choisissez deux versions.",
        "de": "Wähle zwei Versionen.",
        "pt-PT": "Escolha duas versões.",
        "ru": "Выберите две версии.",
        "ja": "2つのバージョンを選択してください。",
        "zh-CN": "请选择两个版本。",
    },
    "{n} files · current version {tag} · last probe {when}": {
        "fr": "{n} fichiers · version actuelle {tag} · dernière sonde {when}",
        "de": "{n} Dateien · aktuelle Version {tag} · letzter Probe {when}",
        "pt-PT": "{n} ficheiros · versão atual {tag} · última verificação {when}",
        "ru": "{n} файлов · текущая версия {tag} · последняя проверка {when}",
        "ja": "{n} ファイル · 現在のバージョン {tag} · 最終確認 {when}",
        "zh-CN": "{n} 个文件 · 当前版本 {tag} · 上次探测 {when}",
    },
    "Version {tag} · {n} change(s)": {
        "fr": "Version {tag} · {n} modification(s)",
        "de": "Version {tag} · {n} Änderung(en)",
        "pt-PT": "Versão {tag} · {n} alteração(ões)",
        "ru": "Версия {tag} · изменений: {n}",
        "ja": "バージョン {tag} · {n} 件の変更",
        "zh-CN": "版本 {tag} · {n} 项更改",
    },
    "Showing the most recent {n} of {t}": {
        "fr": "Affichage des {n} plus récentes sur {t}",
        "de": "Zeige die {n} neuesten von {t}",
        "pt-PT": "A mostrar as {n} mais recentes de {t}",
        "ru": "Показаны последние {n} из {t}",
        "ja": "最新の {n} 件（全 {t} 件中）",
        "zh-CN": "显示最近的 {n} 个，共 {t} 个",
    },
    "{n} version(s)": {
        "fr": "{n} version(s)", "de": "{n} Version(en)",
        "pt-PT": "{n} versão(ões)", "ru": "версий: {n}",
        "ja": "{n} バージョン", "zh-CN": "{n} 个版本",
    },
    "removed in latest capture": {
        "fr": "supprimé dans la dernière capture",
        "de": "in letzter Aufnahme entfernt",
        "pt-PT": "removido na captura mais recente",
        "ru": "удалён в последнем снимке",
        "ja": "最新のキャプチャで削除されました",
        "zh-CN": "已在最新捕获中移除",
    },
    "Click a row to set B; shift-click to set A": {
        "fr": "Cliquez sur une ligne pour définir B ; Shift+clic pour A",
        "de": "Klick auf eine Zeile setzt B; Shift+Klick setzt A",
        "pt-PT": "Clique numa linha para definir B; Shift+clique para A",
        "ru": "Клик по строке — задаёт B; Shift+клик — задаёт A",
        "ja": "行をクリックで B、Shift+クリックで A を設定",
        "zh-CN": "点击行设为 B；Shift+点击设为 A",
    },
    "Failed to load": {
        "fr": "Échec du chargement", "de": "Laden fehlgeschlagen",
        "pt-PT": "Falha ao carregar", "ru": "Ошибка загрузки",
        "ja": "読み込みに失敗", "zh-CN": "加载失败",
    },
    "just now": {"fr": "à l'instant", "de": "gerade eben", "pt-PT": "agora mesmo", "ru": "только что", "ja": "たった今", "zh-CN": "刚刚"},
    "{n}m ago": {"fr": "il y a {n} min", "de": "vor {n} Min", "pt-PT": "há {n} min", "ru": "{n} мин назад", "ja": "{n} 分前", "zh-CN": "{n} 分钟前"},
    "{n}h ago": {"fr": "il y a {n} h", "de": "vor {n} Std", "pt-PT": "há {n} h", "ru": "{n} ч назад", "ja": "{n} 時間前", "zh-CN": "{n} 小时前"},
    "{n}d ago": {"fr": "il y a {n} j", "de": "vor {n} Tag(en)", "pt-PT": "há {n} dia(s)", "ru": "{n} дн назад", "ja": "{n} 日前", "zh-CN": "{n} 天前"},
    "never probed": {"fr": "jamais sondé", "de": "nie geprüft", "pt-PT": "nunca verificado", "ru": "никогда не проверялось", "ja": "未確認", "zh-CN": "尚未探测"},
    "contains changes": {"fr": "contient des modifications", "de": "enthält Änderungen", "pt-PT": "contém alterações", "ru": "содержит изменения", "ja": "変更が含まれます", "zh-CN": "包含更改"},
    "Files differ but no hunks were produced (empty result).": {
        "fr": "Les fichiers diffèrent mais aucun bloc n'a été produit (résultat vide).",
        "de": "Dateien unterscheiden sich, aber keine Hunks erzeugt (leeres Ergebnis).",
        "pt-PT": "Os ficheiros diferem mas não foram produzidos blocos (resultado vazio).",
        "ru": "Файлы отличаются, но блоки не сформированы (пустой результат).",
        "ja": "ファイルは異なりますが、ハンクは生成されませんでした（結果は空）。",
        "zh-CN": "文件不同但未生成差异块（结果为空）。",
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

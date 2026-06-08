"""One-shot: add translation keys for the public user accounts system
(/login, /signup, /dashboard, navbar account widget). Additive —
existing keys are preserved.

Run: `python scripts/merge_site_auth_translations.py`
"""
from __future__ import annotations

import json
from pathlib import Path

TRANSLATIONS: dict[str, dict[str, str]] = {
    # Navbar widget
    "Sign in": {
        "fr": "Connexion", "de": "Anmelden", "pt-PT": "Entrar",
        "ru": "Войти", "ja": "サインイン", "zh-CN": "登录",
    },
    "Sign out": {
        "fr": "Déconnexion", "de": "Abmelden", "pt-PT": "Sair",
        "ru": "Выйти", "ja": "サインアウト", "zh-CN": "退出登录",
    },
    "Dashboard": {
        "fr": "Tableau de bord", "de": "Dashboard", "pt-PT": "Painel",
        "ru": "Панель", "ja": "ダッシュボード", "zh-CN": "仪表盘",
    },

    # Login page
    "Welcome back. Sign in with your username or email.": {
        "fr": "Bon retour. Connectez-vous avec votre nom d'utilisateur ou e-mail.",
        "de": "Willkommen zurück. Mit Benutzername oder E-Mail anmelden.",
        "pt-PT": "Bem-vindo de volta. Entre com nome de utilizador ou e-mail.",
        "ru": "С возвращением. Войдите по имени пользователя или e-mail.",
        "ja": "おかえりなさい。ユーザー名またはメールでサインインしてください。",
        "zh-CN": "欢迎回来。使用用户名或邮箱登录。",
    },
    "Username or email": {
        "fr": "Nom d'utilisateur ou e-mail",
        "de": "Benutzername oder E-Mail",
        "pt-PT": "Nome de utilizador ou e-mail",
        "ru": "Имя пользователя или e-mail",
        "ja": "ユーザー名またはメール",
        "zh-CN": "用户名或邮箱",
    },
    "Password": {
        "fr": "Mot de passe", "de": "Passwort", "pt-PT": "Palavra-passe",
        "ru": "Пароль", "ja": "パスワード", "zh-CN": "密码",
    },
    "No account yet?": {
        "fr": "Pas encore de compte ?", "de": "Noch kein Konto?",
        "pt-PT": "Ainda sem conta?", "ru": "Ещё нет аккаунта?",
        "ja": "アカウントはまだですか？", "zh-CN": "还没有账户？",
    },
    "Create one": {
        "fr": "Créer un compte", "de": "Erstellen",
        "pt-PT": "Criar uma", "ru": "Создать",
        "ja": "作成する", "zh-CN": "立即创建",
    },
    "Resend verification email": {
        "fr": "Renvoyer l'e-mail de vérification",
        "de": "Verifizierungs-E-Mail erneut senden",
        "pt-PT": "Reenviar e-mail de verificação",
        "ru": "Отправить письмо подтверждения снова",
        "ja": "確認メールを再送信",
        "zh-CN": "重新发送验证邮件",
    },
    "Enter your email above first, then click resend.": {
        "fr": "Saisissez d'abord votre e-mail ci-dessus, puis cliquez sur renvoyer.",
        "de": "Trage zuerst deine E-Mail oben ein, dann auf Erneut senden klicken.",
        "pt-PT": "Indique primeiro o seu e-mail acima e depois clique em reenviar.",
        "ru": "Сначала введите e-mail выше, затем нажмите «Отправить снова».",
        "ja": "まず上にメールを入力してから、再送信をクリックしてください。",
        "zh-CN": "请先在上方填写邮箱，然后点击重新发送。",
    },
    "Verification email sent.": {
        "fr": "E-mail de vérification envoyé.",
        "de": "Verifizierungs-E-Mail gesendet.",
        "pt-PT": "E-mail de verificação enviado.",
        "ru": "Письмо подтверждения отправлено.",
        "ja": "確認メールを送信しました。",
        "zh-CN": "已发送验证邮件。",
    },
    "Sign-in failed.": {
        "fr": "Échec de la connexion.", "de": "Anmeldung fehlgeschlagen.",
        "pt-PT": "Falha ao entrar.", "ru": "Не удалось войти.",
        "ja": "サインインに失敗しました。", "zh-CN": "登录失败。",
    },

    # Signup page
    "Create your account": {
        "fr": "Créez votre compte", "de": "Erstelle dein Konto",
        "pt-PT": "Cria a tua conta", "ru": "Создайте аккаунт",
        "ja": "アカウントを作成する", "zh-CN": "创建账户",
    },
    "Pick a username, confirm your email, claim your Trove player name.": {
        "fr": "Choisissez un nom d'utilisateur, confirmez votre e-mail, revendiquez votre nom de joueur Trove.",
        "de": "Wähle einen Benutzernamen, bestätige deine E-Mail, claime deinen Trove-Spielernamen.",
        "pt-PT": "Escolha um nome de utilizador, confirme o e-mail, reivindique o seu nome Trove.",
        "ru": "Выберите имя пользователя, подтвердите e-mail, заявите ваше имя игрока Trove.",
        "ja": "ユーザー名を選び、メールを確認し、Trove のプレイヤー名を取得しましょう。",
        "zh-CN": "选个用户名、验证邮箱、认领你的 Trove 玩家名。",
    },
    "Username": {
        "fr": "Nom d'utilisateur", "de": "Benutzername",
        "pt-PT": "Nome de utilizador", "ru": "Имя пользователя",
        "ja": "ユーザー名", "zh-CN": "用户名",
    },
    "3–24 chars, letters/digits/underscore": {
        "fr": "3–24 caractères, lettres/chiffres/underscore",
        "de": "3–24 Zeichen, Buchstaben/Ziffern/Unterstrich",
        "pt-PT": "3–24 caracteres, letras/dígitos/sublinhado",
        "ru": "3–24 символа, буквы/цифры/подчёркивание",
        "ja": "3–24文字、英数字とアンダースコア",
        "zh-CN": "3–24 字符，字母/数字/下划线",
    },
    "Lowercased on submit. Public-facing — pick something you'd like other players to see.": {
        "fr": "Mis en minuscules à la soumission. Public — choisissez ce que les autres joueurs verront.",
        "de": "Wird beim Senden klein geschrieben. Öffentlich — wähle, was andere Spieler sehen sollen.",
        "pt-PT": "Convertido para minúsculas no envio. Público — escolha o que outros jogadores vão ver.",
        "ru": "Будет переведено в нижний регистр. Публичное — выберите то, что увидят другие игроки.",
        "ja": "送信時に小文字化されます。公開されるので、他プレイヤーに見せたいものを選んでください。",
        "zh-CN": "提交时会小写。公开可见——选个想让其他玩家看到的名字。",
    },
    "Email": {
        "fr": "E-mail", "de": "E-Mail", "pt-PT": "E-mail",
        "ru": "E-mail", "ja": "メール", "zh-CN": "邮箱",
    },
    "We'll send a verification link. Required before you can claim a Trove name.": {
        "fr": "Nous enverrons un lien de vérification. Requis avant de revendiquer un nom Trove.",
        "de": "Wir senden einen Verifizierungs-Link. Erforderlich, bevor du einen Trove-Namen claimen kannst.",
        "pt-PT": "Enviaremos um link de verificação. Necessário antes de reivindicar um nome Trove.",
        "ru": "Мы отправим ссылку подтверждения. Требуется до заявки имени Trove.",
        "ja": "確認リンクをお送りします。Trove 名の取得前に必要です。",
        "zh-CN": "我们会发送验证链接。在认领 Trove 名前需要先验证。",
    },
    "At least 8 characters. We check against known breach lists on submit.": {
        "fr": "Au moins 8 caractères. Vérifié contre les listes de fuites connues.",
        "de": "Mindestens 8 Zeichen. Wir prüfen gegen bekannte Leak-Listen beim Senden.",
        "pt-PT": "Pelo menos 8 caracteres. Verificamos contra listas de fugas conhecidas no envio.",
        "ru": "Минимум 8 символов. Сверяем со списками известных утечек при отправке.",
        "ja": "8文字以上。送信時に既知の流出リストと照合します。",
        "zh-CN": "至少 8 个字符。提交时会与已知泄露列表比对。",
    },
    "Display name": {
        "fr": "Nom d'affichage", "de": "Anzeigename",
        "pt-PT": "Nome a exibir", "ru": "Отображаемое имя",
        "ja": "表示名", "zh-CN": "显示名",
    },
    "(optional)": {
        "fr": "(facultatif)", "de": "(optional)",
        "pt-PT": "(opcional)", "ru": "(необязательно)",
        "ja": "（任意）", "zh-CN": "（可选）",
    },
    "Create account": {
        "fr": "Créer le compte", "de": "Konto erstellen",
        "pt-PT": "Criar conta", "ru": "Создать аккаунт",
        "ja": "アカウントを作成", "zh-CN": "创建账户",
    },
    "Already have an account?": {
        "fr": "Vous avez déjà un compte ?", "de": "Bereits ein Konto?",
        "pt-PT": "Já tens conta?", "ru": "Уже есть аккаунт?",
        "ja": "すでにアカウントをお持ちですか？", "zh-CN": "已有账户？",
    },
    "Sign-up failed.": {
        "fr": "Échec de l'inscription.", "de": "Registrierung fehlgeschlagen.",
        "pt-PT": "Falha no registo.", "ru": "Не удалось зарегистрироваться.",
        "ja": "登録に失敗しました。", "zh-CN": "注册失败。",
    },

    # Dashboard
    "Loading your dashboard…": {
        "fr": "Chargement de votre tableau de bord…",
        "de": "Dashboard wird geladen…",
        "pt-PT": "A carregar o seu painel…",
        "ru": "Загружаем панель…",
        "ja": "ダッシュボードを読み込み中…",
        "zh-CN": "正在加载仪表盘……",
    },
    "Welcome back": {
        "fr": "Bon retour", "de": "Willkommen zurück",
        "pt-PT": "Bem-vindo de volta", "ru": "С возвращением",
        "ja": "おかえりなさい", "zh-CN": "欢迎回来",
    },
    "Welcome back, {n}": {
        "fr": "Bon retour, {n}", "de": "Willkommen zurück, {n}",
        "pt-PT": "Bem-vindo de volta, {n}", "ru": "С возвращением, {n}",
        "ja": "おかえりなさい、{n}さん", "zh-CN": "欢迎回来，{n}",
    },
    "Your account is verified and ready to go.": {
        "fr": "Votre compte est vérifié et prêt à l'emploi.",
        "de": "Dein Konto ist verifiziert und einsatzbereit.",
        "pt-PT": "A sua conta está verificada e pronta.",
        "ru": "Ваш аккаунт подтверждён и готов к работе.",
        "ja": "アカウントは確認済みで利用可能です。",
        "zh-CN": "你的账户已验证，准备就绪。",
    },
    "Confirm your email to unlock the dashboard’s features.": {
        "fr": "Confirmez votre e-mail pour débloquer les fonctionnalités du tableau de bord.",
        "de": "Bestätige deine E-Mail, um die Dashboard-Funktionen freizuschalten.",
        "pt-PT": "Confirme o e-mail para desbloquear as funcionalidades do painel.",
        "ru": "Подтвердите e-mail, чтобы открыть возможности панели.",
        "ja": "メールを確認してダッシュボードの機能を有効化してください。",
        "zh-CN": "请确认邮箱以解锁仪表盘功能。",
    },
    "Confirm your email": {
        "fr": "Confirmez votre e-mail", "de": "Bestätige deine E-Mail",
        "pt-PT": "Confirme o seu e-mail", "ru": "Подтвердите e-mail",
        "ja": "メールを確認", "zh-CN": "确认邮箱",
    },
    "We sent a verification link to your inbox. Confirm it to claim your Trove name and unlock the rest of the dashboard.": {
        "fr": "Nous avons envoyé un lien de vérification dans votre boîte mail. Confirmez-le pour revendiquer votre nom Trove et débloquer le reste du tableau de bord.",
        "de": "Wir haben einen Verifizierungs-Link an deine Mailbox gesendet. Bestätige ihn, um deinen Trove-Namen zu claimen.",
        "pt-PT": "Enviámos um link de verificação para a sua caixa de entrada. Confirme-o para reivindicar o seu nome Trove.",
        "ru": "Мы отправили ссылку подтверждения вам на почту. Подтвердите её, чтобы заявить имя Trove.",
        "ja": "確認リンクを受信箱に送信しました。確認して Trove 名を取得してください。",
        "zh-CN": "我们已向你的邮箱发送验证链接。请确认后即可认领 Trove 名。",
    },
    "Resend link": {
        "fr": "Renvoyer le lien", "de": "Link erneut senden",
        "pt-PT": "Reenviar link", "ru": "Отправить ссылку снова",
        "ja": "リンクを再送信", "zh-CN": "重发链接",
    },
    "Sent — check your inbox": {
        "fr": "Envoyé — vérifiez votre boîte de réception",
        "de": "Gesendet — sieh in dein Postfach",
        "pt-PT": "Enviado — verifica a tua caixa",
        "ru": "Отправлено — проверьте почту",
        "ja": "送信しました — 受信箱をご確認ください",
        "zh-CN": "已发送 — 请查收邮箱",
    },
    "Failed to send": {
        "fr": "Échec de l'envoi", "de": "Senden fehlgeschlagen",
        "pt-PT": "Falha ao enviar", "ru": "Не удалось отправить",
        "ja": "送信に失敗しました", "zh-CN": "发送失败",
    },
    "Profile": {
        "fr": "Profil", "de": "Profil",
        "pt-PT": "Perfil", "ru": "Профиль",
        "ja": "プロフィール", "zh-CN": "个人资料",
    },
    "Member since": {
        "fr": "Membre depuis", "de": "Mitglied seit",
        "pt-PT": "Membro desde", "ru": "Зарегистрирован(а)",
        "ja": "登録日", "zh-CN": "加入时间",
    },
    "Edit": {
        "fr": "Modifier", "de": "Bearbeiten",
        "pt-PT": "Editar", "ru": "Изменить",
        "ja": "編集", "zh-CN": "编辑",
    },
    "(none set)": {
        "fr": "(non défini)", "de": "(nicht gesetzt)",
        "pt-PT": "(não definido)", "ru": "(не задано)",
        "ja": "（未設定）", "zh-CN": "（未设置）",
    },
    "Change password": {
        "fr": "Changer le mot de passe", "de": "Passwort ändern",
        "pt-PT": "Alterar palavra-passe", "ru": "Сменить пароль",
        "ja": "パスワードを変更", "zh-CN": "修改密码",
    },
    "Current password:": {
        "fr": "Mot de passe actuel :", "de": "Aktuelles Passwort:",
        "pt-PT": "Palavra-passe atual:", "ru": "Текущий пароль:",
        "ja": "現在のパスワード：", "zh-CN": "当前密码：",
    },
    "New password (min 8 chars):": {
        "fr": "Nouveau mot de passe (min 8 caractères) :",
        "de": "Neues Passwort (mind. 8 Zeichen):",
        "pt-PT": "Nova palavra-passe (mín. 8 caracteres):",
        "ru": "Новый пароль (мин. 8 символов):",
        "ja": "新しいパスワード（8文字以上）：",
        "zh-CN": "新密码（至少 8 个字符）：",
    },
    "Password updated.": {
        "fr": "Mot de passe mis à jour.", "de": "Passwort aktualisiert.",
        "pt-PT": "Palavra-passe atualizada.", "ru": "Пароль обновлён.",
        "ja": "パスワードを更新しました。", "zh-CN": "密码已更新。",
    },
    "Failed to change password.": {
        "fr": "Échec du changement de mot de passe.",
        "de": "Passwortänderung fehlgeschlagen.",
        "pt-PT": "Falha ao alterar a palavra-passe.",
        "ru": "Не удалось сменить пароль.",
        "ja": "パスワード変更に失敗しました。",
        "zh-CN": "修改密码失败。",
    },
    "New display name (blank to clear):": {
        "fr": "Nouveau nom d'affichage (laisser vide pour effacer) :",
        "de": "Neuer Anzeigename (leer = entfernen):",
        "pt-PT": "Novo nome a exibir (vazio para remover):",
        "ru": "Новое отображаемое имя (пусто — очистить):",
        "ja": "新しい表示名（空欄でクリア）：",
        "zh-CN": "新显示名（留空清除）：",
    },
    "Trove player name": {
        "fr": "Nom de joueur Trove", "de": "Trove-Spielername",
        "pt-PT": "Nome de jogador Trove", "ru": "Имя игрока Trove",
        "ja": "Trove プレイヤー名", "zh-CN": "Trove 玩家名",
    },
    "unverified": {
        "fr": "non vérifié", "de": "unbestätigt",
        "pt-PT": "não verificado", "ru": "не подтверждено",
        "ja": "未確認", "zh-CN": "未验证",
    },
    "Claim your in-game name to surface your leaderboard ranks below. Anyone can claim any name in v1 — verification is coming.": {
        "fr": "Revendiquez votre nom en jeu pour faire apparaître vos rangs de classement ci-dessous. Tout le monde peut revendiquer n'importe quel nom en v1 — la vérification arrive.",
        "de": "Claime deinen In-Game-Namen, um deine Bestenlisten-Ränge unten zu sehen. In v1 darf jeder jeden Namen claimen — Verifizierung kommt noch.",
        "pt-PT": "Reivindique o seu nome no jogo para ver os seus rankings abaixo. Na v1 qualquer um pode reivindicar qualquer nome — a verificação está a caminho.",
        "ru": "Заявите своё имя в игре, чтобы увидеть свои позиции в таблицах ниже. В v1 любой может заявить любое имя — подтверждение позже.",
        "ja": "ゲーム内名を取得すると、下にランキング順位が表示されます。v1 では誰でも任意の名前を取得可能 — 確認機能は今後対応予定。",
        "zh-CN": "认领你的游戏内名字以在下方看到排行榜名次。v1 任何人都可认领任何名字——验证功能即将上线。",
    },
    "Claim": {
        "fr": "Revendiquer", "de": "Claimen",
        "pt-PT": "Reivindicar", "ru": "Заявить",
        "ja": "取得する", "zh-CN": "认领",
    },
    "Failed to claim that name.": {
        "fr": "Échec de la revendication de ce nom.",
        "de": "Name konnte nicht geclaimt werden.",
        "pt-PT": "Falha ao reivindicar esse nome.",
        "ru": "Не удалось заявить это имя.",
        "ja": "この名前の取得に失敗しました。",
        "zh-CN": "认领该名字失败。",
    },
    "You currently have this name claimed:": {
        "fr": "Vous avez actuellement revendiqué ce nom :",
        "de": "Du hast aktuell diesen Namen geclaimt:",
        "pt-PT": "Atualmente reivindicas este nome:",
        "ru": "Сейчас за вами заявлено имя:",
        "ja": "現在取得中の名前：",
        "zh-CN": "当前已认领的名字：",
    },
    "Claimed {when}": {
        "fr": "Revendiqué le {when}", "de": "Geclaimt am {when}",
        "pt-PT": "Reivindicado em {when}", "ru": "Заявлено {when}",
        "ja": "{when} に取得", "zh-CN": "于 {when} 认领",
    },
    "Release name": {
        "fr": "Libérer le nom", "de": "Namen freigeben",
        "pt-PT": "Libertar o nome", "ru": "Освободить имя",
        "ja": "名前を解放", "zh-CN": "释放名字",
    },
    "Release this Trove name? You can re-claim later.": {
        "fr": "Libérer ce nom Trove ? Vous pourrez le re-revendiquer plus tard.",
        "de": "Diesen Trove-Namen freigeben? Du kannst ihn später erneut claimen.",
        "pt-PT": "Libertar este nome Trove? Pode reivindicar mais tarde.",
        "ru": "Освободить это имя Trove? Можно заявить снова позже.",
        "ja": "この Trove 名を解放しますか？後で再取得できます。",
        "zh-CN": "释放此 Trove 名？以后还可以重新认领。",
    },
    "Your leaderboard appearances": {
        "fr": "Vos apparitions au classement",
        "de": "Deine Bestenlisten-Auftritte",
        "pt-PT": "As tuas presenças nas classificações",
        "ru": "Ваши появления в таблицах",
        "ja": "リーダーボード掲載一覧",
        "zh-CN": "你的排行榜出场记录",
    },
    "{n} recent appearance(s)": {
        "fr": "{n} apparition(s) récente(s)",
        "de": "{n} aktuelle Auftritte",
        "pt-PT": "{n} presença(s) recente(s)",
        "ru": "Недавних появлений: {n}",
        "ja": "最近 {n} 件の掲載",
        "zh-CN": "最近 {n} 次出场",
    },
    "No recent leaderboard appearances for this player name yet. The bot captures hourly — check back after the next sweep, or pick a different name.": {
        "fr": "Aucune apparition récente au classement pour ce nom de joueur. Le bot capture toutes les heures — revenez après le prochain passage ou choisissez un autre nom.",
        "de": "Noch keine aktuellen Bestenlisten-Auftritte für diesen Spielernamen. Der Bot erfasst stündlich — schau nach dem nächsten Durchlauf wieder vorbei oder wähle einen anderen Namen.",
        "pt-PT": "Ainda sem presenças recentes para este nome. O bot captura de hora a hora — volta depois da próxima passagem ou escolhe outro nome.",
        "ru": "Пока нет недавних появлений в таблицах для этого имени. Бот собирает данные раз в час — зайдите после следующего прохода или выберите другое имя.",
        "ja": "このプレイヤー名の最近のリーダーボード掲載はまだありません。ボットは1時間ごとに取得します — 次の取得後に再確認するか、別の名前をお試しください。",
        "zh-CN": "暂时没有该玩家名的近期排行榜记录。机器人每小时抓取一次——下次抓取后再来看看，或换个名字。",
    },
    "Could not load your stats right now.": {
        "fr": "Impossible de charger vos statistiques actuellement.",
        "de": "Stats konnten gerade nicht geladen werden.",
        "pt-PT": "Não foi possível carregar as suas estatísticas agora.",
        "ru": "Сейчас не удалось загрузить вашу статистику.",
        "ja": "現在ステータスを読み込めません。",
        "zh-CN": "暂时无法加载你的统计数据。",
    },
    "Board": {
        "fr": "Classement", "de": "Bestenliste",
        "pt-PT": "Classificação", "ru": "Таблица",
        "ja": "ボード", "zh-CN": "榜单",
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

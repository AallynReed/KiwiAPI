"""One-shot: add translation keys for the forgot/reset password pages,
captcha-related copy, and Trove-name verification UI on the dashboard.
Additive — existing keys are preserved.

Run: `python scripts/merge_site_auth_v2_translations.py`
"""
from __future__ import annotations

import json
from pathlib import Path

TRANSLATIONS: dict[str, dict[str, str]] = {
    # /forgot-password
    "Forgot your password?": {
        "fr": "Mot de passe oublié ?",
        "de": "Passwort vergessen?",
        "pt-PT": "Esqueceu a palavra-passe?",
        "ru": "Забыли пароль?",
        "ja": "パスワードをお忘れですか？",
        "zh-CN": "忘记密码？",
    },
    "Enter your account email — we'll send you a link to set a new password.": {
        "fr": "Entrez l'e-mail de votre compte — nous vous enverrons un lien pour définir un nouveau mot de passe.",
        "de": "Gib deine Konto-E-Mail ein — wir senden dir einen Link, um ein neues Passwort zu setzen.",
        "pt-PT": "Indique o e-mail da sua conta — enviaremos um link para definir uma nova palavra-passe.",
        "ru": "Введите e-mail вашего аккаунта — мы пришлём ссылку для смены пароля.",
        "ja": "アカウントのメールを入力してください — 新しいパスワードを設定するリンクをお送りします。",
        "zh-CN": "输入你的账户邮箱——我们会发送设置新密码的链接。",
    },
    "Send reset link": {
        "fr": "Envoyer le lien", "de": "Link senden",
        "pt-PT": "Enviar link", "ru": "Отправить ссылку",
        "ja": "リンクを送信", "zh-CN": "发送链接",
    },
    "Remembered it?": {
        "fr": "Vous vous en souvenez ?", "de": "Doch erinnert?",
        "pt-PT": "Já se lembrou?", "ru": "Вспомнили?",
        "ja": "思い出しましたか？", "zh-CN": "想起来了？",
    },
    "Check your inbox — if that email's registered, a reset link is on its way.": {
        "fr": "Vérifiez votre boîte de réception — si cet e-mail est enregistré, un lien de réinitialisation est en route.",
        "de": "Sieh in dein Postfach — wenn die E-Mail registriert ist, ist ein Reset-Link unterwegs.",
        "pt-PT": "Verifica a tua caixa — se o e-mail estiver registado, um link de redefinição está a caminho.",
        "ru": "Проверьте почту — если этот e-mail зарегистрирован, ссылка для сброса уже в пути.",
        "ja": "受信箱を確認してください — このメールが登録済みなら、リセットリンクが届きます。",
        "zh-CN": "请查收邮箱 — 如果该邮箱已注册，重置链接已发送。",
    },
    "Something went wrong. Try again in a minute.": {
        "fr": "Une erreur est survenue. Réessayez dans une minute.",
        "de": "Etwas ist schiefgelaufen. Versuche es in einer Minute erneut.",
        "pt-PT": "Algo correu mal. Tenta novamente daqui a um minuto.",
        "ru": "Что-то пошло не так. Повторите через минуту.",
        "ja": "問題が発生しました。1分後にもう一度お試しください。",
        "zh-CN": "出了点问题。请稍后再试。",
    },

    # /reset-password
    "Set a new password": {
        "fr": "Définir un nouveau mot de passe",
        "de": "Neues Passwort setzen",
        "pt-PT": "Definir nova palavra-passe",
        "ru": "Установить новый пароль",
        "ja": "新しいパスワードを設定",
        "zh-CN": "设置新密码",
    },
    "Pick something at least 8 characters long. We'll sign every existing session out for safety.": {
        "fr": "Choisissez au moins 8 caractères. Nous déconnecterons toutes vos sessions par sécurité.",
        "de": "Mindestens 8 Zeichen. Wir melden alle bestehenden Sitzungen sicherheitshalber ab.",
        "pt-PT": "Pelo menos 8 caracteres. Vamos sair de todas as sessões existentes por segurança.",
        "ru": "Минимум 8 символов. Все текущие сессии будут завершены для безопасности.",
        "ja": "8文字以上を選んでください。安全のため、既存のすべてのセッションをサインアウトします。",
        "zh-CN": "至少 8 个字符。为安全起见我们会注销所有现有会话。",
    },
    "This reset link is missing its token. Request a fresh one from \"Forgot password\".": {
        "fr": "Ce lien de réinitialisation manque de jeton. Demandez-en un nouveau via « Mot de passe oublié ».",
        "de": "Dieser Reset-Link enthält keinen Token. Fordere einen neuen über „Passwort vergessen“ an.",
        "pt-PT": "Este link de redefinição não tem token. Peça um novo em \"Esqueci a palavra-passe\".",
        "ru": "В этой ссылке для сброса нет токена. Запросите новую через «Забыли пароль».",
        "ja": "このリセットリンクにはトークンがありません。「パスワードをお忘れですか」から新しいリンクをリクエストしてください。",
        "zh-CN": "此重置链接缺少令牌。请通过“忘记密码”重新申请。",
    },
    "New password": {
        "fr": "Nouveau mot de passe", "de": "Neues Passwort",
        "pt-PT": "Nova palavra-passe", "ru": "Новый пароль",
        "ja": "新しいパスワード", "zh-CN": "新密码",
    },
    "Confirm password": {
        "fr": "Confirmer le mot de passe", "de": "Passwort bestätigen",
        "pt-PT": "Confirmar palavra-passe", "ru": "Подтвердите пароль",
        "ja": "パスワードを確認", "zh-CN": "确认密码",
    },
    "Set new password": {
        "fr": "Définir le mot de passe", "de": "Passwort setzen",
        "pt-PT": "Definir palavra-passe", "ru": "Установить пароль",
        "ja": "パスワードを設定", "zh-CN": "设置密码",
    },
    "Back to sign in": {
        "fr": "Retour à la connexion", "de": "Zurück zur Anmeldung",
        "pt-PT": "Voltar ao login", "ru": "К входу",
        "ja": "サインインに戻る", "zh-CN": "返回登录",
    },
    "The two passwords don't match.": {
        "fr": "Les deux mots de passe ne correspondent pas.",
        "de": "Die beiden Passwörter stimmen nicht überein.",
        "pt-PT": "As palavras-passe não coincidem.",
        "ru": "Пароли не совпадают.",
        "ja": "2つのパスワードが一致しません。",
        "zh-CN": "两次输入的密码不一致。",
    },
    "Reset failed.": {
        "fr": "Réinitialisation échouée.", "de": "Reset fehlgeschlagen.",
        "pt-PT": "Redefinição falhou.", "ru": "Сброс не выполнен.",
        "ja": "リセットに失敗しました。", "zh-CN": "重置失败。",
    },
    "Your password has been reset. Redirecting to sign in…": {
        "fr": "Votre mot de passe a été réinitialisé. Redirection vers la connexion…",
        "de": "Dein Passwort wurde zurückgesetzt. Weiterleitung zur Anmeldung…",
        "pt-PT": "A sua palavra-passe foi redefinida. A redirecionar para o login…",
        "ru": "Ваш пароль сброшен. Перенаправление на вход…",
        "ja": "パスワードがリセットされました。サインイン画面に移動します…",
        "zh-CN": "密码已重置。正在跳转到登录页…",
    },
    "Forgot your password?": {
        "fr": "Mot de passe oublié ?",
        "de": "Passwort vergessen?",
        "pt-PT": "Esqueceu a palavra-passe?",
        "ru": "Забыли пароль?",
        "ja": "パスワードをお忘れですか？",
        "zh-CN": "忘记密码？",
    },

    # Trove-name verification UI on the dashboard
    "verified": {
        "fr": "vérifié", "de": "verifiziert",
        "pt-PT": "verificado", "ru": "подтверждено",
        "ja": "確認済み", "zh-CN": "已验证",
    },
    "This claim is unverified — anyone can claim any name until they prove ownership. Score on any board you appear on, then click below.": {
        "fr": "Cette revendication n'est pas vérifiée — n'importe qui peut revendiquer n'importe quel nom jusqu'à preuve de propriété. Marquez des points sur un tableau où vous apparaissez, puis cliquez ci-dessous.",
        "de": "Dieser Claim ist unverifiziert — jeder kann jeden Namen claimen, bis Eigentum bewiesen ist. Erziele Punkte auf einer Liste, auf der du erscheinst, und klicke unten.",
        "pt-PT": "Esta reivindicação não está verificada — qualquer um pode reivindicar qualquer nome até provar a propriedade. Pontue numa classificação onde apareças e clica abaixo.",
        "ru": "Это заявление не подтверждено — любой может заявить любое имя до подтверждения. Заработайте очки на любой таблице, где вы появляетесь, и нажмите ниже.",
        "ja": "この取得は未確認です — 所有権を証明するまでは誰でも任意の名前を取得可能。あなたが掲載されているボードでスコアを稼いでから下のボタンをクリックしてください。",
        "zh-CN": "此认领尚未验证 — 在证明所有权前任何人都可认领任意名字。在你已出现的榜单上得分，然后点击下方。",
    },
    "Verify now": {
        "fr": "Vérifier maintenant", "de": "Jetzt verifizieren",
        "pt-PT": "Verificar agora", "ru": "Подтвердить сейчас",
        "ja": "今すぐ確認", "zh-CN": "立即验证",
    },
    "Checking…": {
        "fr": "Vérification…", "de": "Wird überprüft…",
        "pt-PT": "A verificar…", "ru": "Проверка…",
        "ja": "確認中…", "zh-CN": "正在检查…",
    },
    "Ownership verified": {
        "fr": "Propriété vérifiée", "de": "Eigentum verifiziert",
        "pt-PT": "Propriedade verificada", "ru": "Право собственности подтверждено",
        "ja": "所有権が確認されました", "zh-CN": "所有权已验证",
    },
    "Verified {when}": {
        "fr": "Vérifié le {when}", "de": "Verifiziert am {when}",
        "pt-PT": "Verificado em {when}", "ru": "Подтверждено {when}",
        "ja": "{when} に確認", "zh-CN": "于 {when} 验证",
    },
    "We have a baseline on {n} board(s) — score on any of them to verify.": {
        "fr": "Nous avons une référence sur {n} classement(s) — marquez des points sur l'un d'eux pour vérifier.",
        "de": "Wir haben eine Baseline auf {n} Bestenliste(n) — erziele Punkte auf einer davon, um zu verifizieren.",
        "pt-PT": "Temos uma baseline em {n} classificação(ões) — pontue em qualquer uma para verificar.",
        "ru": "У нас есть базовая отметка на {n} таблице(ах) — заработайте очки на любой, чтобы подтвердить.",
        "ja": "{n}個のボードでベースラインを保持しています — どれかでスコアを稼ぐと確認できます。",
        "zh-CN": "我们已在 {n} 个榜单上记录了基线 — 在其中任一榜单得分即可验证。",
    },
    "We didn't capture any leaderboard data for that name at claim time. Play a bit, then click Verify to re-check.": {
        "fr": "Nous n'avons capturé aucune donnée de classement pour ce nom au moment de la revendication. Jouez un peu, puis cliquez sur Vérifier pour réessayer.",
        "de": "Wir hatten zur Claim-Zeit keine Bestenlisten-Daten für diesen Namen. Spiel ein wenig, dann klicke auf Verifizieren, um erneut zu prüfen.",
        "pt-PT": "Não capturámos dados de classificação para esse nome no momento da reivindicação. Joga um pouco e depois clica em Verificar.",
        "ru": "На момент заявки у нас не было данных по этому имени. Поиграйте немного и нажмите «Подтвердить» снова.",
        "ja": "取得時にその名前のリーダーボードデータはありませんでした。少しプレイしてから「確認」を押してください。",
        "zh-CN": "认领时我们没有该名字的排行榜数据。先玩一会儿，再点击验证。",
    },
    "Verification failed.": {
        "fr": "La vérification a échoué.", "de": "Verifizierung fehlgeschlagen.",
        "pt-PT": "Falha na verificação.", "ru": "Подтверждение не удалось.",
        "ja": "確認に失敗しました。", "zh-CN": "验证失败。",
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

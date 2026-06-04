"""A curated blocklist of common disposable / throwaway email domains. Not
exhaustive — extend as abuse appears. Catches the high-volume providers."""

DISPOSABLE_DOMAINS: frozenset[str] = frozenset(
    {
        "0clock.net", "10minutemail.com", "10minutemail.net", "20minutemail.com",
        "33mail.com", "anonbox.net", "burnermail.io", "byom.de", "deadaddress.com",
        "discard.email", "dispostable.com", "dropmail.me", "emailondeck.com",
        "emailtemporanea.com", "fakeinbox.com", "fakemail.net", "fakemailgenerator.com",
        "getairmail.com", "getnada.com", "grr.la", "guerrillamail.biz",
        "guerrillamail.com", "guerrillamail.de", "guerrillamail.net", "guerrillamail.org",
        "guerrillamailblock.com", "harakirimail.com", "inboxbear.com", "inboxkitten.com",
        "incognitomail.com", "jetable.org", "mail-temp.com", "mail7.io",
        "mailcatch.com", "maildrop.cc", "maileater.com", "mailinator.com",
        "mailinator.net", "mailnesia.com", "mailsac.com", "mailtothis.com",
        "mintemail.com", "mohmal.com", "moakt.com", "mvrht.com", "mytemp.email",
        "nada.email", "nwytg.net", "owlymail.com", "sharklasers.com", "shitmail.org",
        "spam4.me", "spamgourmet.com", "tempinbox.com", "temp-mail.io", "temp-mail.org",
        "tempmail.com", "tempmail.dev", "tempmailo.com", "tempr.email",
        "throwawaymail.com", "trashmail.com", "trashmail.de", "trbvm.com",
        "vomoto.com", "wegwerfmail.de", "yopmail.com", "yopmail.fr", "yopmail.net",
    }
)


def is_disposable_email(email: str) -> bool:
    domain = email.rsplit("@", 1)[-1].lower().strip()
    return domain in DISPOSABLE_DOMAINS

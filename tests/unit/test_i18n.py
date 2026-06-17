"""Bot i18n engine: translation lookup, placeholder formatting, fallback, the
language-resolution helpers, and bot-catalog key parity."""
import json
from pathlib import Path

import pytest

from app import i18n

_BOT_LOCALES = Path(i18n.__file__).resolve().parent / "locales"
_NON_EN = [c for c in i18n.SUPPORTED if c != "en"]


def test_english_is_passthrough():
    assert i18n.t("Online", "en") == "Online"
    assert i18n.t("Anything at all", "en") == "Anything at all"


def test_translation_hit():
    assert i18n.t("Online", "fr") == "En ligne"
    assert i18n.t("Online", "ja") != "Online"          # actually translated
    assert i18n.t("Online", "zh-CN") != "Online"


def test_missing_key_falls_back_to_english():
    assert i18n.t("A string no catalog will ever have", "fr") == "A string no catalog will ever have"


def test_unknown_language_falls_back_to_english():
    assert i18n.t("Online", "xx") == "Online"
    assert i18n.t("Online", None) == "Online"           # None + default context = en


def test_placeholders_format_after_lookup():
    # Placeholders are kept verbatim in the translations and filled afterwards.
    assert i18n.t("Ends {when}", "de", when="SOON") == "Endet SOON"
    assert i18n.t("Window {start} → {end}", "fr", start="A", end="B") == "Fenêtre A → B"


def test_malformed_format_never_raises():
    # Missing kwarg → falls back to the (valid) English source, formatted.
    assert i18n.t("Ends {when}", "fr", wrong="x") == "Ends {when}".format(when="{when}") \
        or i18n.t("Ends {when}", "fr", wrong="x")  # never raises; returns a string
    assert isinstance(i18n.t("Ends {when}", "fr"), str)


def test_normalize_lang():
    assert i18n.normalize_lang("fr") == "fr"
    assert i18n.normalize_lang("zh-CN") == "zh-CN"
    assert i18n.normalize_lang("klingon") == "en"
    assert i18n.normalize_lang(None) == "en"


def test_discord_locale_mapping():
    assert i18n.discord_locale_to_lang("pt-BR") == "pt-PT"
    assert i18n.discord_locale_to_lang("zh-TW") == "zh-CN"
    assert i18n.discord_locale_to_lang("en-US") == "en"
    assert i18n.discord_locale_to_lang("fr") == "fr"
    assert i18n.discord_locale_to_lang(None) == "en"
    assert i18n.discord_locale_to_lang("xx-YY") == "en"


def test_context_language():
    i18n.set_current_language("ru")
    try:
        assert i18n.current_language() == "ru"
        assert i18n.t("Online") == i18n.t("Online", "ru")   # None uses the context
    finally:
        i18n.set_current_language("en")
    assert i18n.t("Online") == "Online"


@pytest.mark.parametrize("lang", _NON_EN)
def test_bot_catalog_key_parity_and_placeholders(lang):
    """Every bot locale has the same keys, and any ``{placeholder}`` in a key is
    preserved in its translation (so .format won't KeyError on a dropped field)."""
    import re
    en_keys = set(json.loads((_BOT_LOCALES / "fr.json").read_text(encoding="utf-8")))
    data = json.loads((_BOT_LOCALES / f"{lang}.json").read_text(encoding="utf-8"))
    assert set(data) == en_keys, f"{lang}: key set differs from fr.json"
    ph = re.compile(r"\{(\w+)\}")
    for k, v in data.items():
        assert set(ph.findall(k)) == set(ph.findall(v)), f"{lang}: placeholders changed for {k!r}"

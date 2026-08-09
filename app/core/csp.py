"""Content-Security-Policy strings for every surface this app serves.

Deliberately dependency-free (no settings, no FastAPI): ``app.core.middleware``
attaches these to real responses, and ``scripts/site_dev_server.py`` imports
them so the dev server serves the SAME policy the edge does. Without that
parity a newly-added inline ``<script>`` works perfectly in dev and silently
dies in production - which is exactly how the CSP regressions in this codebase
have always been found (too late).
"""

# The API serves JSON plus a few small, self-contained HTML pages (landing,
# verify-email, reset-password). Those use inline <style>/<script>, so inline is
# allowed, but everything external is locked down.
API_CSP = (
    "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; "
    "connect-src 'self'; img-src 'self' data:; base-uri 'none'; frame-ancestors 'none'"
)

# The BetterTroveTools showcase site self-hosts all fonts and Font Awesome from
# /static/fonts/ (GDPR: no font/icon request ever leaves our origin to Google or
# cdnjs), and calls the Kiwi API for release data. The captcha widget (Turnstile
# or hCaptcha, whichever the API is configured for) hosts its script + iframe
# under its own domains, so script-src + frame-src cover the union of providers
# and a toggle from one to the other doesn't require a CSP edit.
#
# script-src carries NO 'unsafe-inline': every page script is an external file
# under /static/, and there are no inline handlers (`onclick=`) or `javascript:`
# URLs anywhere in site/templates. That is what makes an injected <script> a
# dead end rather than a session-stealer, so keep it that way - a new inline
# block or `onerror=` attribute will silently stop running in the browser
# (curl won't notice; tests/smoke/test_frontend_smoke.py will).
#
# style-src DOES still allow inline: `style="..."` attributes are pervasive in
# the templates and a nonce cannot cover a style ATTRIBUTE, only a <style>
# block. Closing that one needs 'unsafe-hashes' or an attribute sweep.
SITE_CSP = (
    "default-src 'self'; "
    "script-src 'self' "
        "https://challenges.cloudflare.com https://hcaptcha.com https://*.hcaptcha.com; "
    "style-src 'self' 'unsafe-inline' "
        "https://hcaptcha.com https://*.hcaptcha.com; "
    "font-src 'self' data:; "
    # Allow any https image so user-content READMEs render badges + screenshots
    # (shields.io, github, imgur, …) like GitHub. Images can't execute, so this is
    # low-risk; `data:` covers inline, `cdn.discordapp.com` is already https.
    "img-src 'self' data: https:; "
    # Data-plane binary (mod artifacts, textures, blueprints, VFX assets) is
    # served from the API origin cross-origin; viewers fetch() it (connect-src),
    # but declare media-src too so any <audio>/<video> from the API isn't blocked
    # by the default-src fallback.
    "media-src 'self' https://api.aallyn.net; "
    "connect-src 'self' https://api.aallyn.net "
        "https://challenges.cloudflare.com https://hcaptcha.com https://*.hcaptcha.com; "
    "frame-src https://challenges.cloudflare.com https://hcaptcha.com https://*.hcaptcha.com; "
    "base-uri 'none'; frame-ancestors 'none'"
)

# Everything in the site CSP except its frame-ancestors directive. Split by name
# rather than by position so reordering SITE_CSP can't silently leave the original
# `frame-ancestors 'none'` in front of ours (first directive wins - the embed would
# stay unframable, with a perfectly valid-looking header).
SITE_CSP_NO_FRAME_ANCESTORS = "; ".join(
    d.strip() for d in SITE_CSP.split(";")
    if d.strip() and not d.strip().startswith("frame-ancestors")
)


def embed_csp(origins: list[str]) -> str:
    """Site CSP with `frame-ancestors` swapped for the embed allowlist."""
    ancestors = " ".join(origins) if origins else "'none'"
    return f"{SITE_CSP_NO_FRAME_ANCESTORS}; frame-ancestors {ancestors}"

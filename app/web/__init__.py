"""The BetterTroveTools showcase website (``trove.aallyn.net``), as a separate
presentation-only service.

``app.web.main:app`` renders the HTML pages + serves ``/static`` and holds NO
database connection. Everything dynamic it needs server-side (OG meta, the
browse/clubs/support first-paint, feature-flag gating, the sitemap) it fetches
from the API over HTTP via ``app.core.internal_api.internal_get``. In the
browser, the page JS calls ``api.aallyn.net`` directly for ``/site/*`` + ``/v1/*``
data (CORS-allowed). The data plane - all ``/site/*`` proxies, OG PNG renders and
binary/CAS reads - stays on the API (``app.main``).
"""

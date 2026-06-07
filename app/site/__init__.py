"""BetterTroveTools showcase site, served by the API container.

`trove.aallyn.net` lands on these routes (point the reverse proxy at the same
api container that serves `api.aallyn.net`). The templates live under `site/`
(bind-mounted into the container) so screenshot assets aren't baked into the
Docker image. The `/unlock_*` byte-patcher tools accept a ~100 MB exe upload —
the body-cap exclusion lives in `app/core/middleware.py`.
"""

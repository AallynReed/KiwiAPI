"""Live event stream - a push replacement for polling challenge / chaos-chest.

``bus`` is the cross-worker fan-out (Redis pub/sub + dedup); ``router`` exposes
the SSE endpoint ``GET /v1/events/stream``.
"""

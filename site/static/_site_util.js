/* Shared site utilities (window.BTTUtil). Loaded before every page script so
   pages destructure these instead of re-declaring their own copies. */
(function () {
    "use strict";

    const ESC_MAP = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" };

    // HTML-escape for interpolation into innerHTML. Null/undefined -> "".
    function esc(s) {
        return String(s ?? "").replace(/[&<>"']/g, (c) => ESC_MAP[c]);
    }

    // Same-origin JSON GET, Promise-style: resolves the parsed body, REJECTS with
    // the HTTP status number on a non-2xx. For .then/.catch callers.
    function getJSON(u) {
        return fetch(u, { headers: { Accept: "application/json" } })
            .then((r) => (r.ok ? r.json() : Promise.reject(r.status)));
    }

    // Same-origin JSON GET, async/throw: returns the parsed body, THROWS an Error
    // on a non-2xx whose message is the API's error detail when present (falls back
    // to error.message, then "HTTP <status>"). For await + try/catch callers.
    async function fetchJSON(path) {
        const res = await fetch(path, { headers: { Accept: "application/json" } });
        if (!res.ok) {
            let msg = `HTTP ${res.status}`;
            try {
                const body = await res.json();
                if (body && body.detail) msg = body.detail;
                else if (body && body.error && body.error.message) msg = body.error.message;
            } catch (_) {}
            throw new Error(msg);
        }
        return res.json();
    }

    window.BTTUtil = { esc, getJSON, fetchJSON };
})();

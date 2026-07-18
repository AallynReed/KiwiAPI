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

    // ── Focus management (shared by every modal / dialog on the site) ──────────
    // A dialog must: move focus in, trap Tab inside, and restore focus to the
    // opener on close (WCAG 2.4.3 / 2.1.2). This is the one implementation; the
    // hand-rolled modals + the 3D viewers all route through it.
    const FOCUSABLE = [
        'a[href]', 'area[href]', 'button:not([disabled])',
        'input:not([disabled]):not([type="hidden"])', 'select:not([disabled])',
        'textarea:not([disabled])', '[tabindex]:not([tabindex="-1"])',
        '[contenteditable="true"]', 'summary', 'audio[controls]', 'video[controls]',
    ].join(',');

    // Visible, focusable descendants of `root`, in DOM order.
    function getFocusable(root) {
        if (!root) return [];
        return Array.from(root.querySelectorAll(FOCUSABLE))
            .filter((el) => el.offsetWidth || el.offsetHeight || el.getClientRects().length);
    }

    // Trap focus inside `container`. Returns a release() that removes the trap and
    // (unless returnFocus === false) restores focus to whatever had it before.
    //   opts.onEscape(e)  - called on Escape (typically the caller's close())
    //   opts.initialFocus - element to focus first (defaults to first focusable)
    //   opts.returnFocus  - false to skip focus restoration
    function trapFocus(container, opts) {
        opts = opts || {};
        const prev = document.activeElement;
        const onKey = (e) => {
            if (e.key === "Escape" && opts.onEscape) { opts.onEscape(e); return; }
            if (e.key !== "Tab") return;
            const f = getFocusable(container);
            if (!f.length) { e.preventDefault(); container.focus(); return; }
            const first = f[0], last = f[f.length - 1], active = document.activeElement;
            if (e.shiftKey && (active === first || !container.contains(active))) {
                e.preventDefault(); last.focus();
            } else if (!e.shiftKey && (active === last || !container.contains(active))) {
                e.preventDefault(); first.focus();
            }
        };
        document.addEventListener("keydown", onKey, true);
        const focusInitial = () => {
            const initial = opts.initialFocus || getFocusable(container)[0] || container;
            if (initial === container && !container.hasAttribute("tabindex")) container.setAttribute("tabindex", "-1");
            try { initial.focus(); } catch (_) {}
        };
        // Focus synchronously (works in headless / background tabs where rAF is
        // throttled). If the container was still [hidden] at call time, retry once
        // on the next frame.
        focusInitial();
        if (!container.contains(document.activeElement)) requestAnimationFrame(focusInitial);
        let released = false;
        return function release() {
            if (released) return;
            released = true;
            document.removeEventListener("keydown", onKey, true);
            if (opts.returnFocus !== false && prev && prev.focus) { try { prev.focus(); } catch (_) {} }
        };
    }

    window.BTTUtil = { esc, getJSON, fetchJSON, getFocusable, trapFocus };
})();

/* Shared site utilities (window.BTTUtil). Loaded before every page script so
   pages destructure these instead of re-declaring their own copies. */
(function () {
    "use strict";

    // ── iOS :active enabler ───────────────────────────────────────────────────
    // Mobile Safari only applies :active to an element if it (or an ancestor)
    // has a touch handler bound. Without this no-op listener the press states in
    // style.css section 20 never fire on iPhone - which is the one platform
    // where they matter most, since there is no hover to fall back on.
    document.addEventListener("touchstart", function () {}, { passive: true });

    // ── Cross-origin data base (window.API_BASE) ──────────────────────────────
    // The website is served from its own origin (trove.aallyn.net) but the data
    // plane - every /site/* + /v1/* + /git/* endpoint - lives on the API origin
    // (api.aallyn.net). ``apiUrl`` rewrites those paths onto ``window.API_BASE``.
    //
    // Production hosts (*.aallyn.net) serve the pages from the WEBSITE container,
    // which has no data plane - so /site/* + /v1/* + /git/* must go to the API
    // host. Local dev (localhost / 127.* / bare hostnames, incl. the site_dev
    // preview and the single-process app) stays same-origin ("") so relative paths
    // resolve against whatever is serving the page. A page may pre-set
    // window.API_BASE before this script to override (e.g. dev against staging).
    if (window.API_BASE === undefined) {
        // Exact host or a real subdomain of aallyn.net - NOT "evilaallyn.net".
        const _h = location.hostname;
        window.API_BASE = (_h === "aallyn.net" || _h.endsWith(".aallyn.net"))
            ? "https://api.aallyn.net"
            : "";
    }

    // Rewrite a data-plane path onto the API origin. Static assets (/static/*),
    // in-page fragments (#…) and already-absolute URLs are left untouched, so this
    // is safe to wrap around every URL indiscriminately (and is idempotent).
    function apiUrl(path) {
        if (typeof path === "string" &&
            (path.startsWith("/site/") || path.startsWith("/v1/") || path.startsWith("/git/"))) {
            return window.API_BASE + path;
        }
        return path;
    }

    // Transparently route EVERY programmatic request through apiUrl, so the ~34
    // page scripts' raw fetch()/XHR calls to /site/* + /v1/* reach the API origin
    // without each call site being rewritten. DOM-attribute URLs (<img src>,
    // <a href>) aren't requests, so those are wrapped at their source instead.
    const _origFetch = window.fetch.bind(window);
    window.fetch = function (input, init) {
        if (typeof input === "string") return _origFetch(apiUrl(input), init);
        return _origFetch(input, init);   // Request objects pass through (unused here)
    };
    const _origOpen = XMLHttpRequest.prototype.open;
    XMLHttpRequest.prototype.open = function (method, url) {
        const args = Array.prototype.slice.call(arguments);
        if (typeof url === "string") args[1] = apiUrl(url);
        return _origOpen.apply(this, args);
    };

    const ESC_MAP = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" };

    // HTML-escape for interpolation into innerHTML. Null/undefined -> "".
    function esc(s) {
        return String(s ?? "").replace(/[&<>"']/g, (c) => ESC_MAP[c]);
    }

    // Same-origin JSON GET, Promise-style: resolves the parsed body, REJECTS with
    // the HTTP status number on a non-2xx. For .then/.catch callers.
    function getJSON(u) {
        return fetch(apiUrl(u), { headers: { Accept: "application/json" } })
            .then((r) => (r.ok ? r.json() : Promise.reject(r.status)));
    }

    // Same-origin JSON GET, async/throw: returns the parsed body, THROWS an Error
    // on a non-2xx whose message is the API's error detail when present (falls back
    // to error.message, then "HTTP <status>"). For await + try/catch callers.
    async function fetchJSON(path) {
        const res = await fetch(apiUrl(path), { headers: { Accept: "application/json" } });
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

    // Trailing-edge debounce: returns a wrapper that runs `fn` only once `ms`
    // have passed with no further call. Preserves `this` for method call sites.
    function debounce(fn, ms) {
        let h;
        return function (...a) { clearTimeout(h); h = setTimeout(() => fn.apply(this, a), ms); };
    }

    // Compact "5m ago" / "3h ago" / "2d ago" for feed/card timestamps. Accepts
    // unix seconds or anything Date.parse handles; "" for missing/unparseable.
    // i18n is looked up lazily (i18n.js loads after this file) and uses the
    // "<unit> ago" keys, so the whole phrase stays one translatable unit.
    // NOT the same function as app.js's timeAgo, which is a full
    // Intl.RelativeTimeFormat ladder (year..second) for the releases page.
    function timeAgo(ts) {
        if (!ts) return "";
        const t = typeof ts === "number" ? ts * 1000 : Date.parse(ts);
        if (!t) return "";
        const tr = (s) => (window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s);
        const s = Math.max(0, (Date.now() - t) / 1000);
        if (s < 3600) return Math.floor(s / 60) + tr("m ago");
        if (s < 86400) return Math.floor(s / 3600) + tr("h ago");
        return Math.floor(s / 86400) + tr("d ago");
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

    // ── Gap-aware line segmentation ───────────────────────────────────────────
    // Every time-series on the site plots ONLY the periods we actually captured.
    // Where a stretch has no data the naive polyline joins straight across it,
    // and that straight run reads as a real trend ("prices climbed all week")
    // when in truth nothing was measured. segmentGaps splits a point list into
    //   runs    - contiguous stretches, drawn solid with their normal markers
    //   bridges - [last-before, first-after] pairs spanning a hole, drawn
    //             dashed + dimmed and never given dots
    // so an empty period is visibly an empty period.
    //   points      - array sorted ascending by x
    //   opts.x      - (p) => number; defaults to p.x
    //   opts.step   - expected spacing between neighbours (bucket size / capture
    //                 cadence). Omitted -> inferred from the median delta.
    //   opts.factor - a delta must exceed factor * step to count as a hole (1.5)
    // Returns { runs, bridges, step }. With < 3 points there's no cadence to
    // infer from, so an unspecified step yields one run and no bridges.
    function segmentGaps(points, opts) {
        opts = opts || {};
        const getX = opts.x || ((p) => p.x);
        const factor = opts.factor || 1.5;
        const pts = points || [];
        if (pts.length < 2) return { runs: pts.length ? [pts.slice()] : [], bridges: [], step: null };

        const deltas = [];
        for (let i = 1; i < pts.length; i++) deltas.push(getX(pts[i]) - getX(pts[i - 1]));
        let step = opts.step;
        if (!(step > 0)) {
            if (deltas.length < 2) return { runs: [pts.slice()], bridges: [], step: null };
            const sorted = deltas.slice().sort((a, b) => a - b);
            step = sorted[Math.floor(sorted.length / 2)];
        }
        if (!(step > 0)) return { runs: [pts.slice()], bridges: [], step: null };

        const limit = step * factor;
        const runs = [];
        const bridges = [];
        let run = [pts[0]];
        for (let i = 1; i < pts.length; i++) {
            if (deltas[i - 1] > limit) {
                bridges.push([pts[i - 1], pts[i]]);
                runs.push(run);
                run = [];
            }
            run.push(pts[i]);
        }
        runs.push(run);
        return { runs, bridges, step };
    }

    // ── Leaderboard board icons ───────────────────────────────────────────────
    // Authoritative board→icon map from the game's own ui/leaderboard_icons set,
    // served from the updates CAS we already mirror (via /site/leaderboards/
    // board-icon/<name> - NOT bundled into the repo, so it stays current with the
    // game). Keyed on the STABLE leaderboard uuid, never a fuzzy name match: a
    // board whose uuid isn't covered here (e.g. Bomber Royale, which the game
    // ships no icon for, or a future seasonal board) renders no icon rather than
    // a wrong one. Shared by the /leaderboards player panel and /player page.
    //
    // Class boards live in three parallel uuid ranges - Power Rank (1000+i),
    // Effort (4000+i) and Paragon (5000+i) for class index i - and all share the
    // one class icon the set provides (icon_paragon_<class>). The order mirrors
    // stats._BOARD_CLASS_ORDER (board release order), using the icon set's class
    // tokens (e.g. barbarian, not candybarbarian).
    const LB_PARAGON_CLASS = [
        "knight", "gunslinger", "faetrickster", "dracolyte", "neonninja",
        "barbarian", "icemage", "shadowhunter", "pirate", "boomeranger",
        "tombraiser", "lunarlancer", "revenant", "chloromancer", "dinotamer",
        "vanguardian", "bard", "solarion",
    ];
    const LB_ICON_BY_UUID = {
        // META
        1: "icon_leaderboard_trove_mastery",
        20: "icon_leaderboard_geode_mastery",
        100: "icon_leaderboard_total_mastery",
        999: "icon_leaderboard_meta_power",
        1100: "icon_leaderboard_stats_club_pr",   // Club Power Rank
        50000: "icon_paragon_all",                // Weekly Highest Paragon (all classes)
        // DELVES
        2004: "icon_leaderboard_challenge_depth",
        2001: "icon_leaderboard_challenge_depth",
        2021: "icon_leaderboard_public_depth",
        2024: "icon_leaderboard_public_depth",
        2011: "icon_leaderboard_private_depth",
        2014: "icon_leaderboard_private_depth",
        // STATS
        6: "icon_leaderboard_stats_pvp",
        9: "icon_leaderboard_stats_boxes",
        32000: "icon_leaderboard_stats_worldboss",
        4: "icon_leaderboard_stats_dungeons",
        3: "icon_leaderboard_stats_enemies_killed",
        10: "icon_leaderboard_stats_flux_earned",
        11: "icon_leaderboard_stats_glim_collected",
        33001: "icon_leaderboard_stats_hearts",
        33002: "icon_leaderboard_stats_hearts_sent",
        13: "icon_leaderboard_stats_infinium_mined",
        14: "icon_leaderboard_stats_invaders",
        15: "icon_leaderboard_stats_loot_collector",
        16: "icon_leaderboard_stats_pinatas_looted",
        17: "icon_leaderboard_stats_pinatas_thrown",
        30001: "icon_leaderboard_geode_adventures",
        30004: "icon_leaderboard_geode_egg",
        21005: "icon_leaderboard_stats_experience",
        21012: "icon_leaderboard_stats_club_xp",
        21004: "icon_leaderboard_stats_worldboss",
        // 30002 / 30003 (Bomber Royale): the icon set ships no bomber art -> no icon.
    };

    // Icon filename (no extension) for a board uuid, or null when uncovered.
    function boardIconName(uuid) {
        const inClassRange =
            (uuid >= 1000 && uuid <= 1017) ||
            (uuid >= 4000 && uuid <= 4017) ||
            (uuid >= 5000 && uuid <= 5017);
        if (inClassRange) {
            const cls = LB_PARAGON_CLASS[uuid % 1000];
            return cls ? `icon_paragon_${cls}` : null;
        }
        return LB_ICON_BY_UUID[uuid] || null;
    }

    // <img> for a board's icon, or "" when the board has no mapped icon. alt=""
    // (decorative - the board name sits right beside it).
    function boardIconImg(uuid, cls) {
        const name = boardIconName(uuid);
        // onerror hides the element if the archive somehow lacks this icon, so a
        // broken-image glyph never shows on a tile.
        return name
            ? `<img class="${cls || "lb-board-icon"}" src="${apiUrl("/site/leaderboards/board-icon/" + name)}" alt="" loading="lazy" decoding="async" onerror="this.remove()">`
            : "";
    }

    // Rank crown: gold / silver / bronze for a #1 / #2 / #3 standing, nothing
    // otherwise. Decorative (the numeric rank sits beside it) but carries a title
    // for hover. Shared by the /leaderboards player panel and /player profile.
    function crownHtml(rank) {
        const tier = rank === 1 ? "gold" : rank === 2 ? "silver" : rank === 3 ? "bronze" : null;
        if (!tier) return "";
        return `<i class="board-crown board-crown-${tier} fa-solid fa-crown" title="#${rank}" aria-hidden="true"></i>`;
    }

    // ─── Creator-written translations ──────────────────────────────────
    // A Mods Hub text field ships as an English base plus a {lang: text} map the
    // creator wrote (summary / description / README). These pick the version a
    // given reader should see; surfaces with their own switch (the mod page) pass
    // the reader's pick as `preferred`, cards just follow the site language.
    function siteLang() {
        return document.documentElement.getAttribute("lang") || "en";
    }

    // {lang: text} of the versions that actually exist, English included.
    function textVersions(base, translations) {
        const out = {};
        if (base && base.trim()) out.en = base;
        Object.keys(translations || {}).forEach((c) => {
            const text = translations[c];
            if (text && text.trim()) out[c] = text;
        });
        return out;
    }

    // The version to show: an explicit pick, else the site language, else
    // English, else whatever single language the creator wrote. null if empty.
    function pickLang(versions, preferred) {
        const codes = Object.keys(versions || {});
        if (!codes.length) return null;
        if (preferred && versions[preferred]) return preferred;
        if (versions[siteLang()]) return siteLang();
        return versions.en ? "en" : codes[0];
    }

    // One-liner for surfaces with no switch of their own (cards).
    function localized(base, translations, preferred) {
        const versions = textVersions(base, translations);
        const lang = pickLang(versions, preferred);
        return lang ? versions[lang] : "";
    }

    window.BTTUtil = {
        esc, apiUrl, getJSON, fetchJSON, debounce, timeAgo, getFocusable, trapFocus,
        segmentGaps, boardIconName, boardIconImg, crownHtml,
        siteLang, textVersions, pickLang, localized,
    };
})();

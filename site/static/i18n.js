/* =========================================================================
   Better Trove Tools - docs/landing i18n
   English is the source of truth (lives in the HTML). Each non-English
   locale ships a JSON map of { normalizedEnglishHTML: translatedHTML }.
   Only strings present in the locale map are swapped, so anything not
   translated (buttons, menus, headings) gracefully stays English.
   ========================================================================= */
(function () {
    "use strict";

    // endonyms - language names shown in their own language (never translated)
    const LANGS = [
        ["en", "English", "🇬🇧"],
        ["fr", "Français", "🇫🇷"],
        ["de", "Deutsch", "🇩🇪"],
        ["pt-PT", "Português", "🇵🇹"],
        ["es", "Español", "🇪🇸"],
        ["ru", "Русский", "🇷🇺"],
        ["ja", "日本語", "🇯🇵"],
        ["ko", "한국어", "🇰🇷"],
        ["zh-CN", "简体中文", "🇨🇳"],
    ];
    const SUPPORTED = new Set(LANGS.map((l) => l[0]));
    const STORAGE_KEY = "btt_docs_lang";

    const originals = new WeakMap(); // element -> original innerHTML
    const changed = new Set();       // elements currently showing a translation
    let dict = {};                    // active locale: normEnglish -> translated
    let current = "en";

    const norm = (s) => s.replace(/\s+/g, " ").trim();

    const phOriginals = new WeakMap(); // element -> original placeholder
    const phChanged = new Set();

    const titleOriginals = new WeakMap(); // element -> original title
    const titleChanged = new Set();

    function cacheOriginals() {
        document.querySelectorAll("[data-i18n]").forEach((el) => {
            if (!originals.has(el)) originals.set(el, el.innerHTML);
        });
        document.querySelectorAll("[data-i18n-placeholder]").forEach((el) => {
            if (!phOriginals.has(el)) phOriginals.set(el, el.getAttribute("placeholder") || "");
        });
        document.querySelectorAll("[data-i18n-title]").forEach((el) => {
            if (!titleOriginals.has(el)) titleOriginals.set(el, el.getAttribute("title") || "");
        });
    }

    // Restore every element we previously translated back to its English source.
    function restoreAll() {
        changed.forEach((el) => {
            const orig = originals.get(el);
            if (orig != null) el.innerHTML = orig;
        });
        changed.clear();
        phChanged.forEach((el) => {
            const orig = phOriginals.get(el);
            if (orig != null) el.setAttribute("placeholder", orig);
        });
        phChanged.clear();
        titleChanged.forEach((el) => {
            const orig = titleOriginals.get(el);
            if (orig != null) el.setAttribute("title", orig);
        });
        titleChanged.clear();
    }

    // Apply the active dictionary. CRUCIAL: only elements that actually have a
    // translation are touched - untranslated elements (sidebar menu, buttons,
    // headings) are left completely alone so their event listeners survive.
    function applyDict() {
        restoreAll();
        if (current === "en") return;
        document.querySelectorAll("[data-i18n]").forEach((el) => {
            const orig = originals.get(el);
            if (orig == null) return;
            const translated = dict[norm(orig)];
            if (translated != null && translated !== "") {
                el.innerHTML = translated;
                changed.add(el);
            }
        });
        document.querySelectorAll("[data-i18n-placeholder]").forEach((el) => {
            const orig = phOriginals.get(el);
            if (orig == null) return;
            const translated = dict[norm(orig)];
            if (translated != null && translated !== "") {
                el.setAttribute("placeholder", translated);
                phChanged.add(el);
            }
        });
        document.querySelectorAll("[data-i18n-title]").forEach((el) => {
            const orig = titleOriginals.get(el);
            if (orig == null) return;
            const translated = dict[norm(orig)];
            if (translated != null && translated !== "") {
                el.setAttribute("title", translated);
                titleChanged.add(el);
            }
        });
    }

    async function loadLocale(lang) {
        if (lang === "en") { dict = {}; return; }
        try {
            const res = await fetch(`/static/locales/${lang}.json`, { cache: "no-cache" });
            dict = res.ok ? await res.json() : {};
        } catch (e) {
            console.warn("i18n: failed to load locale", lang, e);
            dict = {};
        }
    }

    async function setLanguage(lang) {
        if (!SUPPORTED.has(lang)) lang = "en";
        current = lang;
        try { localStorage.setItem(STORAGE_KEY, lang); } catch (e) {}
        document.documentElement.setAttribute("lang", lang);
        await loadLocale(lang);
        applyDict();
        // Multiple .lang-select instances may exist (e.g. one in the navbar and
        // one in the mobile sidebar) - sync all of them to the active language.
        document.querySelectorAll(".lang-select").forEach(sel => {
            if (sel.value !== lang) sel.value = lang;
        });
        syncSwitcherFlags();
        // let other scripts (e.g. the release-info line in app.js) re-render
        document.dispatchEvent(new CustomEvent("btt-lang-changed", { detail: { lang } }));
    }

    function pickInitialLanguage() {
        let saved = null;
        try { saved = localStorage.getItem(STORAGE_KEY); } catch (e) {}
        if (saved && SUPPORTED.has(saved)) return saved;
        // best-effort match on the browser language
        const nav = (navigator.language || "").toLowerCase();
        if (nav.startsWith("zh")) return "zh-CN";
        if (nav.startsWith("ja")) return "ja";
        if (nav.startsWith("ko")) return "ko";
        if (nav.startsWith("ru")) return "ru";
        if (nav.startsWith("pt")) return "pt-PT";
        if (nav.startsWith("fr")) return "fr";
        if (nav.startsWith("de")) return "de";
        if (nav.startsWith("es")) return "es";
        return "en";
    }

    function flagFor(code) {
        const hit = LANGS.find(l => l[0] === code);
        return (hit && hit[2]) || "🌐";
    }

    // Keep every custom switcher's trigger flag + active row in sync with the
    // current language (called on build + on every language change).
    function syncSwitcherFlags() {
        document.querySelectorAll(".lang-dd-flag").forEach(s => { s.textContent = flagFor(current); });
        document.querySelectorAll(".lang-dd-item").forEach(it => {
            it.classList.toggle("active", it.dataset.lang === current);
        });
    }

    function buildSwitcher() {
        // Each .lang-select becomes a compact custom dropdown: a flag-only
        // trigger (small, fits the navbar) with the full language NAMES in the
        // open panel. The native <select> is kept (hidden) as a no-JS fallback
        // and so setLanguage's value-sync still has something to write to.
        const selects = document.querySelectorAll(".lang-select");
        if (!selects.length) return;
        selects.forEach(sel => {
            sel.innerHTML = "";
            LANGS.forEach(([code, label]) => {
                const opt = document.createElement("option");
                opt.value = code; opt.textContent = label;
                sel.appendChild(opt);
            });
            sel.hidden = true;
            const host = sel.parentElement || sel;
            if (host.querySelector(".lang-dd")) return;  // already built

            const dd = document.createElement("div");
            dd.className = "lang-dd";
            const trigger = document.createElement("button");
            trigger.type = "button";
            trigger.className = "lang-dd-trigger";
            trigger.setAttribute("aria-haspopup", "true");
            trigger.setAttribute("aria-expanded", "false");
            trigger.setAttribute("aria-label", "Language");
            const flag = document.createElement("span");
            flag.className = "lang-dd-flag";
            const caret = document.createElement("i");
            caret.className = "fa-solid fa-chevron-down lang-dd-caret";
            caret.setAttribute("aria-hidden", "true");
            trigger.appendChild(flag);
            trigger.appendChild(caret);

            const panel = document.createElement("div");
            panel.className = "lang-dd-panel";
            panel.setAttribute("role", "menu");
            panel.hidden = true;
            LANGS.forEach(([code, label, f]) => {
                const item = document.createElement("button");
                item.type = "button";
                item.className = "lang-dd-item";
                item.setAttribute("role", "menuitem");
                item.dataset.lang = code;
                item.textContent = (f ? f + "  " : "") + label;
                item.addEventListener("click", () => { setLanguage(code); close(); });
                panel.appendChild(item);
            });

            function close() { panel.hidden = true; trigger.setAttribute("aria-expanded", "false"); }
            function toggle() {
                const open = panel.hidden;
                panel.hidden = !open;
                trigger.setAttribute("aria-expanded", String(open));
            }
            trigger.addEventListener("click", e => { e.stopPropagation(); toggle(); });
            document.addEventListener("click", e => { if (!dd.contains(e.target)) close(); });
            document.addEventListener("keydown", e => { if (e.key === "Escape") close(); });

            dd.appendChild(trigger);
            dd.appendChild(panel);
            host.appendChild(dd);
        });
        syncSwitcherFlags();
    }

    function init() {
        cacheOriginals();
        buildSwitcher();
        setLanguage(pickInitialLanguage());
    }

    if (document.readyState === "loading") {
        document.addEventListener("DOMContentLoaded", init);
    } else {
        init();
    }

    // Translate a single English string at runtime - for content built in JS
    // (the [data-i18n] sweep only covers strings that already exist in the
    // DOM at apply time).
    function translate(s) {
        if (current === "en") return s;
        const hit = dict[norm(s)];
        return (hit != null && hit !== "") ? hit : s;
    }

    // Re-cache + re-apply translations. Call this after injecting markup
    // with [data-i18n] / [data-i18n-placeholder] nodes so they pick up the
    // active language without a full reload.
    function refresh() {
        cacheOriginals();
        applyDict();
    }

    // Drop an element from all internal tracking. Use this when a node
    // that USED to hold a chrome string is repurposed to hold runtime
    // data (e.g. a board name), so the next applyDict's restoreAll
    // doesn't clobber the runtime text by resetting to the cached
    // English original. The caller is responsible for also removing
    // the [data-i18n] attribute before assigning the new content.
    function untrack(el) {
        if (!el) return;
        originals.delete(el);
        changed.delete(el);
        phOriginals.delete(el);
        phChanged.delete(el);
        titleOriginals.delete(el);
        titleChanged.delete(el);
    }

    // expose for debugging / external triggers. `langs` is the canonical
    // [code, endonym, flag] table - page scripts that build their own language
    // UI (e.g. the Mods Hub translation switch) read it instead of re-listing.
    window.BTTi18n = { setLanguage, t: translate, refresh, untrack, langs: LANGS, current: () => current };
})();

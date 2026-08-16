/* =========================================================================
   Better Trove Tools - docs/landing i18n
   English is the source of truth (lives in the HTML). Each non-English
   locale ships a JSON map of { normalizedEnglishHTML: translatedHTML }.
   Only strings present in the locale map are swapped, so anything not
   translated (buttons, menus, headings) gracefully stays English.

   Mark a string for translation with one of:
     data-i18n              the element's innerHTML
     data-i18n-placeholder  its placeholder attribute
     data-i18n-title        its title attribute
     data-i18n-aria-label   its aria-label - a control whose tooltip
                            translates while its accessible name does not
                            is localised for everyone except the people
                            reading it with a screen reader
   Content built in JS has no element to mark, so it calls BTTi18n.t()
   instead - and, since t() is a one-shot substitution, re-renders itself
   on the `btt-lang-changed` event.
   ========================================================================= */
(function () {
    "use strict";

    // endonyms - language names shown in their own language (never translated)
    const LANGS = [
        ["en", "English"],
        ["fr", "Français"],
        ["de", "Deutsch"],
        ["pt-PT", "Português"],
        ["es", "Español"],
        ["ru", "Русский"],
        ["ja", "日本語"],
        ["ko", "한국어"],
        ["zh-CN", "简体中文"],
        ["th", "ไทย"],
    ];
    const SUPPORTED = new Set(LANGS.map((l) => l[0]));
    const STORAGE_KEY = "btt_docs_lang";

    const originals = new WeakMap(); // element -> original innerHTML
    const changed = new Set();       // elements currently showing a translation
    let dict = {};                    // active locale: normEnglish -> translated
    let current = "en";

    const norm = (s) => s.replace(/\s+/g, " ").trim();

    // Attributes that carry a translatable string, as [marker, attribute]. One
    // table rather than a block per attribute: every one behaves identically -
    // cache the English, swap it when a translation exists, put it back on the
    // way out - so a new one is a line here, not another copy of the machinery.
    //
    // aria-label is the reason this is a table: a control whose visible tooltip
    // translated while its accessible name stayed English left screen-reader
    // users on the only copy of the label that never got localised.
    const ATTRS = [
        ["data-i18n-placeholder", "placeholder"],
        ["data-i18n-title", "title"],
        ["data-i18n-aria-label", "aria-label"],
    ];

    const attrOriginals = new WeakMap(); // element -> { attribute: original English }
    const attrChanged = new Set();       // elements currently showing a translation

    function cacheOriginals() {
        document.querySelectorAll("[data-i18n]").forEach((el) => {
            if (!originals.has(el)) originals.set(el, el.innerHTML);
        });
        ATTRS.forEach(([marker, attr]) => {
            document.querySelectorAll(`[${marker}]`).forEach((el) => {
                const seen = attrOriginals.get(el) || {};
                if (seen[attr] != null) return;          // already cached
                seen[attr] = el.getAttribute(attr) || "";
                attrOriginals.set(el, seen);
            });
        });
    }

    // Restore every element we previously translated back to its English source.
    function restoreAll() {
        changed.forEach((el) => {
            const orig = originals.get(el);
            if (orig != null) el.innerHTML = orig;
        });
        changed.clear();
        attrChanged.forEach((el) => {
            const seen = attrOriginals.get(el);
            if (!seen) return;
            Object.keys(seen).forEach((attr) => {
                if (seen[attr] != null) el.setAttribute(attr, seen[attr]);
            });
        });
        attrChanged.clear();
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
        ATTRS.forEach(([marker, attr]) => {
            document.querySelectorAll(`[${marker}]`).forEach((el) => {
                const orig = (attrOriginals.get(el) || {})[attr];
                if (orig == null) return;
                const translated = dict[norm(orig)];
                if (translated != null && translated !== "") {
                    el.setAttribute(attr, translated);
                    attrChanged.add(el);
                }
            });
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
        document.querySelectorAll("select.lang-select").forEach(sel => {
            if (sel.value !== lang) sel.value = lang;
        });
        syncSwitchers();
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
        if (nav.startsWith("th")) return "th";
        return "en";
    }

    // Short badge for a locale: the region half is dropped because the
    // language half is what people read ("pt-PT" -> PT, "zh-CN" -> ZH).
    // Flag emoji were the obvious choice here and the wrong one: Windows has
    // no flag glyphs, so every trigger rendered as a pair of boxed letters of
    // a different width per language.
    function codeFor(code) {
        return code.split("-")[0].toUpperCase();
    }

    // Keep every custom switcher's trigger badge + active row in sync with the
    // current language (called on build + on every language change).
    function syncSwitchers() {
        document.querySelectorAll(".lang-dd-code").forEach(s => { s.textContent = codeFor(current); });
        document.querySelectorAll(".lang-dd-item").forEach(it => {
            it.classList.toggle("active", it.dataset.lang === current);
            it.setAttribute("aria-checked", String(it.dataset.lang === current));
        });
    }

    function buildSwitcher() {
        // Each .lang-select becomes a compact custom dropdown: a globe + code
        // trigger (small, fits the navbar) with the full language NAMES in the
        // open panel. The native <select> is kept (hidden) as a no-JS fallback
        // and so setLanguage's value-sync still has something to write to.
        //
        // `select.` matters: dropdown.js copies a select's classes onto the
        // button it builds, so a bare ".lang-select" also matches that button
        // and this would fill a <button> with <option>s. The picker carries
        // data-no-dropdown to keep the two enhancers apart in the first place.
        const selects = document.querySelectorAll("select.lang-select");
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
            const globe = document.createElement("i");
            globe.className = "fa-solid fa-globe lang-dd-globe";
            globe.setAttribute("aria-hidden", "true");
            const badge = document.createElement("span");
            badge.className = "lang-dd-code";
            const caret = document.createElement("i");
            caret.className = "fa-solid fa-chevron-down lang-dd-caret";
            caret.setAttribute("aria-hidden", "true");
            trigger.appendChild(globe);
            trigger.appendChild(badge);
            trigger.appendChild(caret);

            const panel = document.createElement("div");
            panel.className = "lang-dd-panel";
            panel.setAttribute("role", "menu");
            panel.hidden = true;
            LANGS.forEach(([code, label]) => {
                const item = document.createElement("button");
                item.type = "button";
                item.className = "lang-dd-item";
                item.setAttribute("role", "menuitemradio");
                item.dataset.lang = code;
                const tag = document.createElement("span");
                tag.className = "lang-dd-tag";
                tag.textContent = codeFor(code);
                const name = document.createElement("span");
                name.textContent = label;
                item.appendChild(tag);
                item.appendChild(name);
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
            document.addEventListener("keydown", e => {
                if (e.key !== "Escape" || panel.hidden) return;
                close();
                trigger.focus();
            });

            dd.appendChild(trigger);
            dd.appendChild(panel);
            host.appendChild(dd);
        });
        syncSwitchers();
    }

    /* Resolves once the FIRST dictionary is in place. `t()` is a one-shot
       substitution, so a page that builds itself in JS and paints before the locale
       file has landed renders every string in English and has no idea it did - the
       `btt-lang-changed` event comes too late for anything that already ran, and
       re-rendering on it is only an option for a view cheap enough to rebuild. A
       page whose boot is expensive (the dashboard refetches the session and every
       section) awaits this instead and paints once, correctly. Never rejects: a
       missing locale file leaves English on screen, which is the fallback anyway. */
    let markReady;
    const ready = new Promise((resolve) => { markReady = resolve; });

    function init() {
        cacheOriginals();
        buildSwitcher();
        setLanguage(pickInitialLanguage()).catch(() => {}).then(() => markReady());
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

    // Re-cache + re-apply translations. Call this after injecting markup with
    // [data-i18n] / [data-i18n-placeholder] / [data-i18n-title] /
    // [data-i18n-aria-label] nodes so they pick up the active language without
    // a full reload.
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
        attrOriginals.delete(el);
        attrChanged.delete(el);
    }

    // expose for debugging / external triggers. `langs` is the canonical
    // [code, endonym, flag] table - page scripts that build their own language
    // UI (e.g. the Mods Hub translation switch) read it instead of re-listing.
    window.BTTi18n = { setLanguage, t: translate, refresh, untrack, langs: LANGS,
                       current: () => current, ready };
})();

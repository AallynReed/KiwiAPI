/* Dressing Room (/dressing-room).

   Pick a class, a costume and up to three equipment styles; the server assembles the
   character out of the game's own parts and `model_viewer.js` draws it - the same
   viewer the Mods Hub uses for an assembled creature.

   The whole outfit lives in the query string, so a look is shared by copying the URL
   and nothing is ever stored. Selections update the URL with replaceState, so a
   session of clicking around leaves one history entry, not fifty.

   Renders are coalesced and sequenced: the controls answer immediately, the model
   follows, and a response that arrives after a newer pick is dropped rather than
   painted over it. */
(function () {
    "use strict";

    var U = window.BTTUtil || {};
    var esc = U.esc || function (s) { return String(s == null ? "" : s); };

    var SLOTS = [
        { id: "costume", label: "Costume", icon: "fa-shirt" },
        { id: "hat", label: "Hat", icon: "fa-hat-wizard" },
        { id: "face", label: "Face", icon: "fa-masks-theater" },
        { id: "weapon", label: "Weapon", icon: "fa-gavel" }
    ];
    var PAGE = 60;
    var RENDER_DEBOUNCE = 220;

    var els = {};
    var classes = [];
    var state = { cls: "", costume: "", hat: "", face: "", weapon: "" };
    var slot = "costume";
    var listing = { items: [], total: 0, offset: 0, q: "", slot: "", cls: "" };
    var chosenNames = {};                 // slot -> display name, so the summary reads
    var viewer = null;                    // even while the model is still loading
    var renderSeq = 0;
    var renderTimer = null;

    function byId(id) { return document.getElementById(id); }

    function classOf(key) {
        for (var i = 0; i < classes.length; i++) {
            if (classes[i].key === key) return classes[i];
        }
        return null;
    }

    /* ── URL state ───────────────────────────────────────────────────────── */

    function readUrl() {
        var q = new URLSearchParams(location.search);
        state.cls = (q.get("class") || "").toLowerCase();
        state.costume = (q.get("costume") || "").toLowerCase();
        state.hat = (q.get("hat") || "").toLowerCase();
        state.face = (q.get("face") || "").toLowerCase();
        state.weapon = (q.get("weapon") || "").toLowerCase();
    }

    function query() {
        var q = new URLSearchParams();
        q.set("class", state.cls);
        if (state.costume) q.set("costume", state.costume);
        ["hat", "face", "weapon"].forEach(function (s) {
            if (state[s]) q.set(s, state[s]);
        });
        return q;
    }

    function writeUrl() {
        history.replaceState(null, "", location.pathname + "?" + query().toString());
    }

    /* ── data ────────────────────────────────────────────────────────────── */

    function get(path) {
        return fetch(path, { headers: { Accept: "application/json" } }).then(function (r) {
            if (!r.ok) throw new Error("HTTP " + r.status);
            return r.json();
        });
    }

    function loadClasses() {
        return get("/site/dressing/classes").then(function (data) {
            classes = (data && data.items) || [];
            if (!classes.length) throw new Error("empty");
            if (!classOf(state.cls)) state.cls = classes[0].key;
            els.cls.innerHTML = classes.map(function (c) {
                return '<option value="' + esc(c.key) + '">' + esc(c.name) + "</option>";
            }).join("");
            els.cls.value = state.cls;
        });
    }

    function optionsUrl(s, offset) {
        var q = new URLSearchParams({ slot: s, offset: String(offset), limit: String(PAGE) });
        if (s === "costume" || s === "weapon") q.set("class", state.cls);
        if (listing.q) q.set("q", listing.q);
        return "/site/dressing/options?" + q.toString();
    }

    function loadOptions(reset) {
        var offset = reset ? 0 : listing.items.length;
        var s = slot, cls = state.cls, q = listing.q;
        return get(optionsUrl(s, offset)).then(function (page) {
            // A slower response for a slot/class/search we've since left must not paint.
            if (s !== slot || cls !== state.cls || q !== listing.q) return;
            listing.items = reset ? page.items : listing.items.concat(page.items);
            listing.total = page.total;
            listing.slot = s;
            listing.cls = cls;
            adoptNames(page.items);
            renderGrid();
        }).catch(function () {
            if (s === slot) renderGridError();
        });
    }

    /* A shared link carries keys, not names, so the summary would read
       `knight_dragon` until the matching tab happened to be opened. Fill the label in
       from whatever list we just fetched. */
    function adoptNames(items) {
        for (var i = 0; i < items.length; i++) {
            if (items[i].key && items[i].key === state[items[i].slot]) {
                chosenNames[items[i].slot] = items[i].name;
                renderCurrent();
            }
        }
    }

    /* Every class wears SOMETHING - with no costume in the URL the server dresses it in
       the first one, so adopt that as the explicit selection rather than showing "None"
       next to a model that is plainly wearing clothes. */
    function ensureCostume() {
        if (state.costume) return Promise.resolve();
        var cls = state.cls;
        return get("/site/dressing/options?slot=costume&class=" +
                   encodeURIComponent(cls) + "&offset=0&limit=1").then(function (page) {
            if (cls !== state.cls || state.costume || !page.items.length) return;
            state.costume = page.items[0].key;
            chosenNames.costume = page.items[0].name;
            writeUrl();
            renderCurrent();
            if (slot === "costume") renderGrid();
        }).catch(function () { /* the model still renders; the chip just stays generic */ });
    }

    /* ── option grid ─────────────────────────────────────────────────────── */

    function thumbUrl(opt) {
        var q = new URLSearchParams({ dim: "96" });
        if (opt.slot === "costume") q.set("prefab", opt.prefab);
        else q.set("blueprint", opt.blueprint);
        return "/site/dressing/render?" + q.toString();
    }

    function card(opt, selected) {
        var img = U.apiUrl ? U.apiUrl(thumbUrl(opt)) : thumbUrl(opt);
        return '<button type="button" class="dr-opt' + (selected ? " sel" : "") +
            '" role="option" aria-selected="' + (selected ? "true" : "false") +
            '" data-key="' + esc(opt.key) + '" data-name="' + esc(opt.name) + '">' +
            '<span class="dr-optimg"><img src="' + esc(img) + '" alt="" loading="lazy" ' +
            'decoding="async" onerror="this.remove()"></span>' +
            '<span class="dr-optname">' + esc(opt.name) + "</span>" +
            (opt.family ? '<span class="dr-optfam">' + esc(opt.family) + "</span>" : "") +
            "</button>";
    }

    function renderGrid() {
        var none = slot === "costume" ? "" :
            '<button type="button" class="dr-opt dr-none' + (state[slot] ? "" : " sel") +
            '" role="option" aria-selected="' + (state[slot] ? "false" : "true") +
            '" data-key="" data-name="None"><span class="dr-optimg dr-noneimg">' +
            '<i class="fa-solid fa-ban" aria-hidden="true"></i></span>' +
            '<span class="dr-optname">None</span></button>';
        var html = listing.items.map(function (o) {
            return card(o, o.key === state[slot]);
        }).join("");
        els.grid.innerHTML = none + (html || (none ? "" :
            '<p class="dr-empty">No matching options.</p>'));
        els.more.hidden = listing.items.length >= listing.total;
    }

    function renderGridError() {
        els.grid.innerHTML = '<p class="dr-empty">Couldn’t load these options. ' +
            "Your selection is still here — try again in a moment.</p>";
        els.more.hidden = true;
    }

    /* ── current-look summary ────────────────────────────────────────────── */

    function renderCurrent() {
        var cls = classOf(state.cls);
        els.current.innerHTML = SLOTS.map(function (s) {
            var key = state[s.id];
            var name = key ? (chosenNames[s.id] || key)
                : (s.id === "costume" ? "Default" : "None");
            var off = (s.id === "weapon" && cls && !cls.weapons.length);
            return '<button type="button" class="dr-chip' + (key ? " on" : "") +
                (off ? " dr-chip-off" : "") + '" data-slot="' + s.id + '">' +
                '<i class="fa-solid ' + s.icon + '" aria-hidden="true"></i>' +
                '<span class="dr-chipslot">' + esc(s.label) + "</span>" +
                '<span class="dr-chipname">' + esc(name) + "</span></button>";
        }).join("");
    }

    function renderTabs() {
        els.tabs.innerHTML = SLOTS.map(function (s) {
            return '<button type="button" role="tab" class="dr-tab' +
                (s.id === slot ? " on" : "") + '" aria-selected="' +
                (s.id === slot ? "true" : "false") + '" data-slot="' + s.id + '">' +
                '<i class="fa-solid ' + s.icon + '" aria-hidden="true"></i> ' +
                '<span data-i18n>' + esc(s.label) + "</span></button>";
        }).join("");
    }

    /* ── the model ───────────────────────────────────────────────────────── */

    function modelUrl() {
        return "/site/dressing/model?" + query().toString();
    }

    function renderModel() {
        var seq = ++renderSeq;
        if (viewer) { viewer.dispose(); viewer = null; }
        els.stage.innerHTML = "";
        els.bar.innerHTML = "";
        viewer = window.ModelViewer.mount(els.stage, {
            url: U.apiUrl ? U.apiUrl(modelUrl()) : modelUrl(),
            bar: els.bar,
            onMeta: function (text) { if (seq === renderSeq) els.meta.textContent = text; }
        });
    }

    function scheduleRender() {
        // Clicking through a gallery fires a pick per thumbnail; only the one you
        // stop on is worth assembling.
        if (renderTimer) clearTimeout(renderTimer);
        renderTimer = setTimeout(function () {
            renderTimer = null;
            renderModel();
        }, RENDER_DEBOUNCE);
    }

    /* ── interactions ────────────────────────────────────────────────────── */

    function pick(key, name) {
        if (state[slot] === key) return;
        state[slot] = key;
        chosenNames[slot] = name;
        writeUrl();
        renderGrid();
        renderCurrent();
        scheduleRender();
    }

    function setSlot(next) {
        if (slot === next) return;
        slot = next;
        listing = { items: [], total: 0, offset: 0, q: "", slot: next, cls: state.cls };
        els.search.value = "";
        renderTabs();
        els.grid.innerHTML = '<p class="dr-empty">Loading…</p>';
        loadOptions(true);
    }

    function setClass(key) {
        state.cls = key;
        // A costume belongs to one class and the weapon families change with it, so
        // both are cleared rather than silently carried onto a body they don't fit.
        state.costume = "";
        state.weapon = "";
        chosenNames.costume = chosenNames.weapon = "";
        writeUrl();
        renderCurrent();
        listing = { items: [], total: 0, offset: 0, q: "", slot: slot, cls: key };
        els.search.value = "";
        els.grid.innerHTML = '<p class="dr-empty">Loading…</p>';
        loadOptions(true);
        ensureCostume();
        scheduleRender();
    }

    function share() {
        var url = location.href;
        var done = function (ok) {
            els.share.classList.toggle("ok", ok);
            els.share.querySelector("span").textContent = ok ? "Link copied" : "Copy failed";
            setTimeout(function () {
                els.share.classList.remove("ok");
                els.share.querySelector("span").textContent = "Copy link";
            }, 2000);
        };
        if (navigator.clipboard && navigator.clipboard.writeText) {
            navigator.clipboard.writeText(url).then(function () { done(true); },
                function () { done(false); });
        } else {
            done(false);
        }
    }

    function wire() {
        els.cls.addEventListener("change", function () { setClass(els.cls.value); });
        els.tabs.addEventListener("click", function (e) {
            var b = e.target.closest("[data-slot]");
            if (b) setSlot(b.getAttribute("data-slot"));
        });
        els.current.addEventListener("click", function (e) {
            var b = e.target.closest("[data-slot]");
            if (b) setSlot(b.getAttribute("data-slot"));
        });
        els.grid.addEventListener("click", function (e) {
            var b = e.target.closest("[data-key]");
            if (b) pick(b.getAttribute("data-key"), b.getAttribute("data-name") || "");
        });
        els.more.addEventListener("click", function () { loadOptions(false); });
        els.share.addEventListener("click", share);
        var run = function () { listing.q = els.search.value.trim(); loadOptions(true); };
        els.search.addEventListener("input", U.debounce ? U.debounce(run, 250) : run);
    }

    function fail(msg) {
        els.stage.innerHTML = '<p class="dr-empty">' + esc(msg) + "</p>";
    }

    function init() {
        els = {
            stage: byId("drStage"), bar: byId("drBar"), meta: byId("drMeta"),
            share: byId("drShare"), cls: byId("drClass"), current: byId("drCurrent"),
            tabs: byId("drTabs"), search: byId("drSearch"), grid: byId("drGrid"),
            more: byId("drMore")
        };
        if (!els.stage) return;
        renderTabs();
        renderCurrent();
        readUrl();
        loadClasses().then(function () {
            wire();
            writeUrl();
            renderCurrent();
            loadOptions(true);
            ensureCostume();
            renderModel();
        }).catch(function () {
            fail("The dressing room isn’t available right now.");
        });
    }

    if (document.readyState === "loading") {
        document.addEventListener("DOMContentLoaded", init);
    } else {
        init();
    }
})();

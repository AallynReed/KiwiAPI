/* ═══════════════════════════════════════════════════════════════════════
   /gems-guide - interactive "How Gems Work in Trove" explainer.
   Vanilla JS, no deps, CSP-clean. All gem numbers mirror the server-side
   gem model (app/trove/gems/{constants,bases}.py) so the guide stays true
   to the actual game data. Client-only; nothing is fetched.
   ═══════════════════════════════════════════════════════════════════════ */
(function () {
  "use strict";
  var doc = document, body = doc.body;
  var REDUCE = window.matchMedia && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  if (REDUCE) body.classList.add("gg-reduce");

  /* ── tiny DOM helper ────────────────────────────────────────────────── */
  function el(tag, attrs, kids) {
    var n = doc.createElement(tag), k;
    if (attrs) for (k in attrs) {
      if (k === "class") n.className = attrs[k];
      else if (k === "html") n.innerHTML = attrs[k];
      else if (k === "text") n.textContent = attrs[k];
      else if (k.slice(0, 5) === "data-" || k === "role" || k === "aria-label" || k.slice(0, 5) === "aria-") n.setAttribute(k, attrs[k]);
      else n[k] = attrs[k];
    }
    if (kids != null) (Array.isArray(kids) ? kids : [kids]).forEach(function (c) {
      if (c != null) n.appendChild(typeof c === "string" ? doc.createTextNode(c) : c);
    });
    return n;
  }
  function $(sel, ctx) { return (ctx || doc).querySelector(sel); }

  /* ── i18n for JS-built content ──────────────────────────────────────────
     The [data-i18n] sweep only covers static markup, so JS-rendered strings
     go through window.BTTi18n.t(). tt() = plain lookup; ttf() = lookup a
     template then fill {placeholders}. Sections register a re-render via
     onLang() so a mid-page language switch updates them too. */
  function tt(s) { return (window.BTTi18n && window.BTTi18n.t) ? window.BTTi18n.t(s) : s; }
  function ttf(s, map) { return tt(s).replace(/\{(\w+)\}/g, function (_, k) { return (map && map[k] != null) ? map[k] : "{" + k + "}"; }); }
  var RERENDER = [];
  function onLang(fn) { RERENDER.push(fn); }
  doc.addEventListener("btt-lang-changed", function () { RERENDER.forEach(function (f) { try { f(); } catch (e) {} }); });

  /* ── game data (mirrors constants.py / bases.py) ────────────────────── */
  var COL = { blue: "#58a6ff", orange: "#ff8a3d", yellow: "#ffd166", purple: "#a371f7", green: "#3fb950", red: "#f85149", white: "#eaf1fa", cyan: "#2fd4e6" };

  // Tier accent colours match the in-game tiers: Radiant white, Stellar gold,
  // Crystal cyan, Mystic purple.
  var TIERS = [
    { id: 1, name: "Radiant", max: 23, pr: 3, color: COL.white,  band: "85 – 113",  note: "Entry tier - the gems you start earning." },
    { id: 2, name: "Stellar", max: 25, pr: 5, color: COL.yellow, band: "150 – 200", note: "A solid mid-game jump over Radiant." },
    { id: 3, name: "Crystal", max: 30, pr: 7, color: COL.cyan,   band: "210 – 280", note: "Where serious end-game power begins." },
    { id: 4, name: "Mystic",  max: 35, pr: 9, color: COL.purple, band: "270 – 360", note: "The ceiling. Every perfect build is Mystic." }
  ];

  var ELEM_ABIL = ["Stinging Curse", "Volatile Velocity", "Spirit Surge", "Mired Mojo", "Stunburst", "Pyrodisc", "Explosive Epilogue", "Cubic Curtain"];
  var COSMIC_ABIL = ["Berserk Battler", "Empyrean Barrier", "Flower Power", "Vampirian Vanquisher"];

  var ELEMENTS = [
    { id: 1, name: "Water", color: COL.blue,   icon: "fa-droplet", slot: "Water socket",  abil: ELEM_ABIL },
    { id: 2, name: "Fire",  color: COL.orange, icon: "fa-fire",    slot: "Fire socket",   abil: ELEM_ABIL },
    { id: 3, name: "Air",   color: COL.yellow, icon: "fa-wind",    slot: "Air socket",    abil: ELEM_ABIL },
    { id: 4, name: "Cosmic",color: COL.purple, icon: "fa-meteor",  slot: "Cosmic socket", abil: COSMIC_ABIL, cosmic: true }
  ];
  function elemById(id) { for (var i = 0; i < ELEMENTS.length; i++) if (ELEMENTS[i].id === id) return ELEMENTS[i]; return null; }

  var POOL = ["Physical Damage", "Critical Damage", "Critical Hit", "Max Health", "Max Health %"];
  var POOL_MAGIC = ["Magic Damage", "Critical Damage", "Critical Hit", "Max Health", "Max Health %"];
  var STAT_ICON = {
    "Physical Damage": "fa-hand-fist", "Magic Damage": "fa-wand-sparkles",
    "Critical Damage": "fa-burst", "Critical Hit": "fa-crosshairs",
    "Max Health": "fa-heart", "Max Health %": "fa-heart-circle-plus", "Light": "fa-sun"
  };
  var AUG = { 1: { name: "Rough", val: 2.5 }, 2: { name: "Precise", val: 5 }, 3: { name: "Superior", val: 12.5 } };

  // Per-level Power-Rank increment - a faithful port of bases.get_level_pr_increment.
  function prInc(level, base) {
    if (level === 1 || level === 5 || level === 10 || level === 15) return 0;
    if (level > 15 && level % 5 === 0) return base * 5;
    if (level > 1 && level < 15) return base;
    if (level > 15) return base * 2;
    return 0;
  }
  function totalLevelPR(tier, upto) {
    var s = 0, cap = upto || tier.max;
    for (var l = 2; l <= cap; l++) s += prInc(l, tier.pr);
    return s;
  }

  /* ── Power Rank of a perfect gem (mirrors Gem.power_rank) ─────────────────
     PR high-threshold = what one full (100%) container is worth, per tier, by
     type. Total = base(+100 if Empowered) + threshold×containers + 3×Σincrements.
     Lesser thresholds = bases._LESSER_PR_THRESHOLD[*][1]; Empowered = _EMPOWERED. */
  var PR_HI = { lesser: { 1: 113, 2: 200, 3: 250, 4: 260 }, emp: { 1: 150, 2: 266, 3: 280, 4: 300 } };
  function gemContainers(level) { return 3 + Math.floor(Math.min(level, 15) / 5); }
  function gemPR(tier, kind, level) {   // kind: "lesser" | "emp"
    return Math.round((kind === "emp" ? 100 : 0) + PR_HI[kind][tier.id] * gemContainers(level) + 3 * totalLevelPR(tier, level));
  }

  /* ── Per-stat value tables (mirrors Gem.stat_values) ─────────────────────
     value = stat_base × (threshold × containers + Σincrements). A stat has
     containers = 1 + boosts (0-3). Stat bases + thresholds from bases.py. */
  var STAT_COLS = [
    { key: "dmg", label: "PD/MD", cat: "dmg", dec: 0 },
    { key: "cd", label: "CD", cat: "crit", dec: 2 },
    { key: "ch", label: "CH", cat: "crit", dec: 2 },
    { key: "mhp", label: "MH%", cat: "health", dec: 2 },
    { key: "mh", label: "MH", cat: "health", dec: 0 },
    { key: "lt", label: "LT", cat: "light", dec: 0 }
  ];
  var STAT_BASE = {
    1: { dmg: 14, cd: 0.2, ch: 0.02, mhp: 0.5, mh: 50, lt: 1 },
    2: { dmg: 14, cd: 0.2, ch: 0.02, mhp: 0.5, mh: 50, lt: 1 },
    3: { dmg: 16, cd: 3 / 14, ch: 0.3 / 14, mhp: 0.5, mh: 50, lt: 5 / 7 },
    4: { dmg: 168 / 9, cd: 2.5 / 9, ch: 0.25 / 9, mhp: 5.25 / 9, mh: 525 / 9, lt: 5 / 9 }
  };
  var THRESH = {
    lesser: {
      1: { all: [85, 113] }, 2: { all: [150, 200] },
      3: { dmg: [210, 280], crit: [560 / 3, 770 / 3], health: [245, 315], light: [280, 385] },
      4: { dmg: [270, 360], crit: [187.2, 297], health: [315, 405], light: [495, 585] }
    },
    emp: {
      1: { all: [113, 150] }, 2: { all: [200, 266] },
      3: { dmg: [245, 350], crit: [700 / 3, 910 / 3], health: [315, 385], light: [350, 420] },
      4: { dmg: [210, 300], crit: [252, 342], health: [405, 495], light: [495, 630] }
    }
  };
  function statBase(tierId, kind, key) {
    if (tierId === 4 && key === "dmg" && kind === "emp") return 28;   // Mystic Empowered damage
    return STAT_BASE[tierId][key];
  }
  function statThresh(tierId, kind, cat) {
    var row = THRESH[kind][tierId];
    return row.all ? row.all : row[cat];
  }
  function prCum(tier, level) { var s = 0, l; for (l = 1; l <= level; l++) s += prInc(l, tier.pr); return s; }
  function statValue(tier, kind, col, level, boosts) {
    var base = statBase(tier.id, kind, col.key), th = statThresh(tier.id, kind, col.cat),
        containers = 1 + boosts, cum = prCum(tier, level);
    return [base * (th[0] * containers + cum), base * (th[1] * containers + cum)];
  }

  /* ── real gem art (the same voxel renders the Gem Simulator uses) ───────
     A gem is a tier "socket" image with the element/type gem composited on
     top - identical to /gem-simulator's layering. Assets live under
     /static/assets/gems/{gem_tiers,gem_types,augments}. */
  var GEM_ASSET = "/static/assets/gems";
  function gemSrc(type, element) { return GEM_ASSET + "/gem_types/" + type + "/elements/" + element + ".png"; }
  function tierSrc(tier) { return GEM_ASSET + "/gem_tiers/" + tier + ".png"; }
  function augSrc(id) { return GEM_ASSET + "/augments/" + id + ".png"; }

  // opts: {tier, type, element, float, alt}. Any of tier/type+element may be
  // omitted (e.g. an empty socket before the gem is chosen).
  function mountGem(stage, opts) {
    if (!stage) return;
    opts = opts || {};
    stage.innerHTML =
      '<div class="gg-gem-render' + (opts.float ? " gg-gem-float" : "") + '">' +
        (opts.tier ? '<img class="gg-gem-tier" src="' + tierSrc(opts.tier) + '" alt="" aria-hidden="true">' : "") +
        '<img class="gg-gem-img" src="' + (opts.type && opts.element ? gemSrc(opts.type, opts.element) : "") + '" alt="' + (opts.alt || "") + '"' + (opts.type && opts.element ? "" : ' hidden') + '>' +
      "</div>";
  }
  function updateGem(stage, opts) {
    if (!stage) return;
    var tierImg = stage.querySelector(".gg-gem-tier"), gemI = stage.querySelector(".gg-gem-img");
    if (opts.tier && tierImg) tierImg.src = tierSrc(opts.tier);
    if (opts.type && opts.element && gemI) { gemI.src = gemSrc(opts.type, opts.element); gemI.hidden = false; if (opts.alt != null) gemI.alt = opts.alt; }
  }
  function setGemColor(stage, color) { if (stage) stage.style.setProperty("--gem", color); }

  /* ── count-up animation ─────────────────────────────────────────────── */
  function countTo(node, to, opts) {
    opts = opts || {};
    var from = opts.from || 0, dur = REDUCE ? 0 : (opts.dur != null ? opts.dur : 700), suffix = opts.suffix || "", start = null;
    if (dur === 0) { node.textContent = Math.round(to) + suffix; return; }
    function frame(t) {
      if (start === null) start = t;
      var p = Math.min(1, (t - start) / dur), e = 1 - Math.pow(1 - p, 3);
      node.textContent = Math.round(from + (to - from) * e) + suffix;
      if (p < 1) requestAnimationFrame(frame);
    }
    requestAnimationFrame(frame);
  }

  /* ═══════════════════════════════════════════════════════════════════
     Static gems (hero + type)
     ═══════════════════════════════════════════════════════════════════ */
  var heroStage = $("#hero-gem");
  if (heroStage) { setGemColor(heroStage, COL.purple); mountGem(heroStage, { type: 2, element: 4, float: true, alt: "A Mystic Empowered Cosmic gem" }); }

  /* ═══════════════════════════════════════════════════════════════════
     1. Scrollytelling — assemble a gem
     ═══════════════════════════════════════════════════════════════════ */
  (function () {
    var stage = $("#scrolly-gem"), spec = $("#scrolly-spec"), steps = Array.prototype.slice.call(doc.querySelectorAll(".gg-step"));
    if (!stage || !spec || !steps.length) return;
    // No tier socket here (the low-res socket art looks poor upscaled) - just the
    // gem, which starts desaturated and colours in once its element locks.
    mountGem(stage, { type: 2, element: 4, float: true, alt: "A Mystic Empowered Cosmic gem" });
    function specVals() { return [tt("Mystic"), tt("Empowered"), tt("Cosmic"), tt("Magic · Crit Dmg · Light"), "Vampirian Vanquisher"]; }
    var rows = Array.prototype.slice.call(spec.querySelectorAll(".gg-spec-row"));
    var current = -1;

    function apply(step) {
      if (step === current) return;
      var forward = step > current, SPEC = specVals();
      current = step;
      steps.forEach(function (s, i) {
        s.classList.toggle("active", i === step);
        s.classList.toggle("done", i < step);
      });
      rows.forEach(function (row, i) {
        var lit = i <= step;
        var dd = row.querySelector("dd");
        if (lit && dd.textContent !== SPEC[i]) { dd.textContent = SPEC[i]; if (forward) { row.classList.add("just-lit"); setTimeout(function () { row.classList.remove("just-lit"); }, 500); } }
        if (!lit) dd.textContent = "—";
        row.classList.toggle("lit", lit);
      });
      // The gem colours in once the element (step 2) locks.
      var colored = step >= 2;
      setGemColor(stage, colored ? COL.purple : "#5a6472");
      stage.setAttribute("data-assembled", colored ? "1" : "0");
    }
    apply(0);

    // Scroll-driven: the step whose vertical centre is nearest the viewport
    // centre is active. Deterministic (fires on real 'scroll' events, works in
    // headless renderers) and gap-proof - there is always exactly one active step.
    function pickStep() {
      var vc = (window.innerHeight || 0) / 2, best = 0, bestDist = Infinity;
      steps.forEach(function (s, i) {
        var r = s.getBoundingClientRect(), c = r.top + r.height / 2, d = Math.abs(c - vc);
        if (d < bestDist) { bestDist = d; best = i; }
      });
      apply(best);
    }
    window.addEventListener("scroll", pickStep, { passive: true });
    window.addEventListener("resize", pickStep);
    window.addEventListener("load", pickStep);
    pickStep();
    onLang(function () { var sv = specVals(); rows.forEach(function (row, i) { if (row.classList.contains("lit")) row.querySelector("dd").textContent = sv[i]; }); });
  })();

  /* ═══════════════════════════════════════════════════════════════════
     2. Tiers
     ═══════════════════════════════════════════════════════════════════ */
  (function () {
    var wrap = $("#gg-tiers"), detail = $("#gg-tier-detail");
    if (!wrap || !detail) return;
    var maxTotal = gemPR(TIERS[3], "emp", TIERS[3].max);   // Mystic Empowered ceiling, for the bar scale
    var active = 3;

    function buildCards() {
      wrap.innerHTML = "";
      TIERS.forEach(function (t, i) {
        var card = el("button", {
          class: "gg-tier", role: "radio", "aria-checked": i === active ? "true" : "false", type: "button"
        }, [
          el("span", { class: "gg-tier-name" }, [el("span", { class: "gg-tier-dot" }), tt(t.name)]),
          el("span", { class: "gg-tier-rank", text: ttf("TIER {n} / 4", { n: t.id }) }),
          el("span", { class: "gg-tier-ml", html: ttf("Levels to <b>{n}</b>", { n: t.max }) })
        ]);
        card.style.setProperty("--tc", t.color);
        if (i === active) card.classList.add("active");
        card.addEventListener("click", function () { select(i); });
        wrap.appendChild(card);
      });
    }

    function select(i) {
      active = i;
      Array.prototype.forEach.call(wrap.children, function (c, k) {
        c.classList.toggle("active", k === i);
        c.setAttribute("aria-checked", k === i ? "true" : "false");
      });
      render();
    }
    function render() {
      var t = TIERS[active], maxPR = gemPR(t, "emp", t.max);
      detail.style.setProperty("--gem", t.color);
      detail.innerHTML = "";
      detail.appendChild(stat(tt("Max level"), t.max, tt(t.note)));
      detail.appendChild(stat(tt("Each new roll"), "+" + PR_HI.emp[t.id], tt("The PR a milestone roll adds (level 5 / 10 / 15).")));
      var barCell = stat(tt("Max Power Rank"), maxPR.toLocaleString(), tt("A perfect Empowered gem at max level."));
      var bar = el("div", { class: "gg-tier-bar" }, el("i"));
      barCell.appendChild(bar);
      detail.appendChild(barCell);
      requestAnimationFrame(function () { bar.firstChild.style.width = Math.round(maxPR / maxTotal * 100) + "%"; });
    }
    function stat(k, v, s) {
      return el("div", { class: "gg-tier-stat" }, [
        el("span", { class: "k", text: k }),
        el("span", { class: "v", text: String(v) }),
        el("span", { class: "s", text: s })
      ]);
    }
    buildCards(); render();
    onLang(function () { buildCards(); render(); });
  })();

  /* ═══════════════════════════════════════════════════════════════════
     2b. Gem Converters — raise a gem's tier, keeping level + augments.
     Crystal Converter: Stellar→Crystal.  Mystic Converter: Crystal→Mystic.
     ═══════════════════════════════════════════════════════════════════ */
  (function () {
    var wrap = $("#gg-conv-cards");
    if (!wrap) return;
    var ICON = "/static/assets/gems/misc/items/";
    var CONV = [
      { img: "crystal_converter", name: "Crystal Converter", from: 1, to: 2, credits: 1000, cubits: 10000 },
      { img: "mystic_converter", name: "Mystic Converter", from: 2, to: 3, credits: 1500, cubits: 15000 }
    ];
    function money(n) { return n.toLocaleString("en-US"); }
    function tierChip(t) {
      var chip = el("span", { class: "gg-conv-tier" }, [
        el("span", { class: "gg-conv-dot" }), tt(t.name)
      ]);
      chip.style.setProperty("--tc", t.color);
      return chip;
    }
    function build() {
      wrap.innerHTML = "";
      CONV.forEach(function (c) {
        var from = TIERS[c.from], to = TIERS[c.to];
        var card = el("div", { class: "gg-conv-card" }, [
          el("img", { class: "gg-conv-ic", src: ICON + c.img + ".png", alt: "", "aria-hidden": "true", loading: "lazy", width: "52", height: "52" }),
          el("div", { class: "gg-conv-body" }, [
            el("span", { class: "gg-conv-name", text: c.name }),
            el("span", { class: "gg-conv-flow" }, [
              tierChip(from),
              el("i", { class: "fa-solid fa-arrow-right gg-conv-arrow", "aria-hidden": "true" }),
              tierChip(to)
            ]),
            el("span", { class: "gg-conv-keep", text: tt("Keeps level + augments") }),
            el("span", { class: "gg-conv-price" }, [
              el("i", { class: "fa-solid fa-coins gg-conv-coin", "aria-hidden": "true" }),
              money(c.credits) + " Credits",
              el("span", { class: "gg-conv-or", text: tt("or") }),
              money(c.cubits) + " Cubits"
            ])
          ])
        ]);
        card.style.setProperty("--gc", to.color);
        wrap.appendChild(card);
      });
    }
    build();
    onLang(build);
  })();

  /* ═══════════════════════════════════════════════════════════════════
     6b. Gem Level Up Boosters (verbatim from the game's item strings:
     languages/en/prefabs_item_gem_booster.binfab). Ordered weakest→strongest.
     Names are item proper nouns, kept in English like Gem Dust / Chaos Spark.
     ═══════════════════════════════════════════════════════════════════ */
  (function () {
    var wrap = $("#gg-boost-cards");
    if (!wrap) return;
    var ICON = "/static/assets/gems/misc/items/";
    // `str` = real strength rank. Ninth & Tenth are the SAME booster (identical
    // numbers) split by tier, so they tie at the top - the meter must not rank
    // Tenth above Ninth.
    var BOOSTERS = [
      { img: "glittering_horseshoe", name: "Glittering Horseshoe", up: 50, dbl: 0, mystic: false, str: 1 },
      { img: "lapis_luckbug", name: "Lapis Luckbug", up: 300, dbl: 500, mystic: false, str: 2 },
      { img: "ninth_life", name: "Ninth Life", up: 4000, dbl: 1000, mystic: false, str: 3 },
      { img: "tenth_life", name: "Tenth Life", up: 4000, dbl: 1000, mystic: true, str: 3 }
    ];
    var MAXSTR = BOOSTERS.reduce(function (m, b) { return Math.max(m, b.str); }, 0);
    function chip(cls, k, v) {
      return el("div", { class: "gg-boost-chip " + cls }, [
        el("span", { class: "gg-boost-chip-k", text: k }),
        el("span", { class: "gg-boost-chip-v", text: v })
      ]);
    }
    function build() {
      wrap.innerHTML = "";
      BOOSTERS.forEach(function (b, i) {
        var meter = el("div", { class: "gg-boost-meter" }, el("i"));
        var tag = b.mystic
          ? el("span", { class: "gg-boost-badge", text: tt("Mystic only") })
          : (b.str === MAXSTR ? el("span", { class: "gg-boost-badge alt", text: tt("All but Mystic") }) : null);
        var card = el("div", { class: "gg-boost-card" + (b.mystic ? " mystic" : "") }, [
          el("div", { class: "gg-boost-top" }, [
            el("img", { class: "gg-boost-ic", src: ICON + b.img + ".png", alt: "", "aria-hidden": "true", loading: "lazy", width: "44", height: "44" }),
            el("div", { class: "gg-boost-id" }, [
              el("span", { class: "gg-boost-name", text: b.name }),
              tag
            ])
          ]),
          el("div", { class: "gg-boost-chips" }, [
            chip("up", tt("Level Up"), "+" + b.up.toLocaleString() + "%"),
            chip("dbl", tt("Double Level Up"), "+" + b.dbl.toLocaleString() + "%")
          ]),
          el("div", { class: "gg-boost-strength" }, [
            el("span", { class: "gg-boost-strength-lbl", text: tt("Strength") }),
            meter
          ])
        ]);
        wrap.appendChild(card);
        (function (m, w) { requestAnimationFrame(function () { m.firstChild.style.width = w + "%"; }); })(meter, b.str / MAXSTR * 100);
      });
    }
    build();
    onLang(build);
  })();

  /* ═══════════════════════════════════════════════════════════════════
     3. Lesser vs Empowered
     ═══════════════════════════════════════════════════════════════════ */
  (function () {
    var segs = Array.prototype.slice.call(doc.querySelectorAll(".gg-type-toggle .gg-seg")),
        detail = $("#gg-type-detail"), gem = $("#type-gem");
    if (!segs.length || !detail) return;
    if (gem) { setGemColor(gem, COL.orange); mountGem(gem, { type: 1, element: 2, float: true, alt: "A Lesser Fire gem" }); }

    var DATA = {
      1: {
        name: "Lesser Gem", badge: "gg-badge-blue", badgeText: "Restricted",
        blurb: "The common gem. It's locked to one damage school and has no special ability - but you slot lots of them, so their combined stats matter.",
        feats: [
          ["fa-lock", "Carries a <b>restriction</b>: Fierce gems roll physical stats, Arcane gems roll magic."],
          ["fa-dice-d6", "Two or three stats, each rolled to a <b>random</b> strength."],
          ["fa-arrows-up-to-line", "Focus the stats to close the gap to a perfect 100%."]
        ]
      },
      2: {
        name: "Empowered Gem", badge: "gg-badge-purple", badgeText: "Unrestricted",
        blurb: "The rare, powerful gem. It drops the damage-school restriction, rolls in a higher band, and carries a special ability - so a single one is worth far more.",
        feats: [
          ["fa-bolt", "Carries a special <b>ability</b> (or a class ability on class gems)."],
          ["fa-arrow-up-right-dots", "Rolls in a <b>higher stat band</b> and starts <b>+100 Power Rank</b> ahead."],
          ["fa-star", "You only equip a few - each one is a major upgrade."]
        ]
      }
    };
    var curType = 1;
    function select(type) {
      curType = type;
      segs.forEach(function (s) { var on = +s.getAttribute("data-type") === type; s.classList.toggle("active", on); s.setAttribute("aria-checked", on ? "true" : "false"); });
      var d = DATA[type];
      detail.style.setProperty("--gem", type === 2 ? COL.purple : COL.orange);
      detail.innerHTML = "";
      detail.appendChild(el("h3", {}, [tt(d.name), el("span", { class: "gg-badge " + d.badge, text: tt(d.badgeText) })]));
      detail.appendChild(el("p", { text: tt(d.blurb) }));
      var list = el("div", { class: "gg-feat-list" });
      d.feats.forEach(function (f, i) {
        var row = el("div", { class: "gg-feat" }, [el("i", { class: "fa-solid " + f[0], "aria-hidden": "true" }), el("span", { html: tt(f[1]) })]);
        row.style.setProperty("--i", i);
        row.style.animationDelay = (REDUCE ? 0 : i * 60) + "ms";
        list.appendChild(row);
      });
      detail.appendChild(list);
      if (gem) updateGem(gem, { type: type, element: 2, alt: (type === 2 ? "An Empowered" : "A Lesser") + " Fire gem" });
    }
    segs.forEach(function (s) { s.addEventListener("click", function () { select(+s.getAttribute("data-type")); }); });
    select(1);
    onLang(function () { select(curType); });
  })();

  /* ═══════════════════════════════════════════════════════════════════
     4. Elements
     ═══════════════════════════════════════════════════════════════════ */
  (function () {
    var wrap = $("#gg-elements"), detail = $("#gg-element-detail");
    if (!wrap || !detail) return;
    // Water/Fire/Air are identical - same rolls, same abilities, only the socket
    // differs - so they collapse into a single card. Cosmic stands alone.
    var GROUPS = [
      { key: "elemental", ids: [1, 2, 3], color: COL.blue,   abil: ELEM_ABIL,   cosmic: false },
      { key: "cosmic",    ids: [4],       color: COL.purple, abil: COSMIC_ABIL, cosmic: true }
    ];
    var active = "elemental";
    function groupByKey(k) { for (var i = 0; i < GROUPS.length; i++) if (GROUPS[i].key === k) return GROUPS[i]; return GROUPS[0]; }
    function groupName(g) { return g.ids.map(function (id) { return tt(elemById(id).name); }).join(" · "); }

    function buildEls() {
      wrap.innerHTML = "";
      GROUPS.forEach(function (g) {
        var orb = el("span", { class: "gg-elem-orb" + (g.ids.length > 1 ? " multi" : "") },
          g.ids.map(function (id) { var im = doc.createElement("img"); im.src = gemSrc(1, id); im.alt = ""; im.loading = "lazy"; return im; }));
        var b = el("button", { class: "gg-elem", role: "radio", type: "button", "aria-checked": g.key === active ? "true" : "false" }, [
          orb,
          el("span", { class: "gg-elem-name", text: groupName(g) }),
          el("span", { class: "gg-elem-slot", text: g.cosmic ? ttf("{el} socket", { el: tt("Cosmic") }) : tt("Three elemental sockets") })
        ]);
        b.style.setProperty("--ec", g.color);
        if (g.key === active) b.classList.add("active");
        b.addEventListener("click", function () { select(g.key); });
        wrap.appendChild(b);
      });
    }

    function tag(name, locked) {
      return el("span", { class: "gg-tag" + (locked ? " locked" : "") }, [
        locked ? el("i", { class: "fa-solid fa-lock", "aria-hidden": "true" }) : null, name
      ]);
    }
    function select(key) {
      active = key;
      Array.prototype.forEach.call(wrap.children, function (c, i) {
        var on = GROUPS[i] && GROUPS[i].key === key; c.classList.toggle("active", on); c.setAttribute("aria-checked", on ? "true" : "false");
      });
      var g = groupByKey(key);
      detail.style.setProperty("--gem", g.color);
      detail.className = "gg-element-detail" + (g.cosmic ? " cosmic" : "");
      detail.innerHTML = "";

      var rolls = el("div", { class: "gg-ed-block" }, el("h3", { html: '<i class="fa-solid fa-dice-d6"></i> ' + tt("Can roll") }));
      var rrow = el("div", { class: "gg-tag-row" });
      ["Damage", "Critical Damage", "Critical Hit", "Max Health", "Max Health %"].forEach(function (n) { rrow.appendChild(tag(tt(n), false)); });
      if (g.cosmic) rrow.appendChild(tag(tt("Light"), true));
      rolls.appendChild(rrow);

      var abil = el("div", { class: "gg-ed-block" }, el("h3", { html: '<i class="fa-solid fa-bolt"></i> ' + tt("Empowered abilities") }));
      var arow = el("div", { class: "gg-tag-row" });
      g.abil.forEach(function (n) { arow.appendChild(tag(n, false)); });
      abil.appendChild(arow);

      detail.appendChild(rolls);
      detail.appendChild(abil);

      var note = g.cosmic
        ? tt('<b>Cosmic is the special one.</b> One stat slot is always locked to <b>Light</b> - the stat that powers Geode &amp; cosmic content - and its Empowered abilities are a set of their own. Cosmic gems go in their own three sockets on top of your elemental gems.')
        : tt('Water, Fire and Air share the exact same rolls and ability set - only the socket they fit differs. Pick the element your class and slots call for.');
      detail.appendChild(el("p", { class: "gg-ed-note" + (g.cosmic ? " cosmic" : ""), html: note }));
      detail.appendChild(el("p", { class: "gg-ed-note gg-ed-primo", html:
        '<i class="fa-solid fa-dragon" aria-hidden="true"></i> ' + tt('Each element has its own <b>Primordial Dragon</b> that boosts <b>every gem of that element by +10%</b>. Water, Fire, Air and Cosmic each have one - unlock all four and your whole gem loadout gets the +10%.') }));
    }
    buildEls(); select("elemental");
    onLang(function () { buildEls(); select(active); });
  })();

  /* ═══════════════════════════════════════════════════════════════════
     5. Stats & rolls
     ═══════════════════════════════════════════════════════════════════ */
  (function () {
    var slotsWrap = $("#gg-roll-slots"), poolList = $("#gg-pool-list"), hint = $("#gg-roll-hint"), btn = $("#gg-reroll");
    if (!slotsWrap) return;
    var gem = COL.blue;

    function buildPool() {
      poolList.innerHTML = "";
      POOL.forEach(function (n) { poolList.appendChild(el("li", {}, [el("span", { class: "d" }), tt(n)])); });
    }
    buildPool();

    var lastNames = null, lastQs = null;
    function grade(q) {
      if (q >= 100) return ["Perfect", "q-high"];
      if (q >= 90) return ["Great roll", "q-high"];
      if (q >= 72) return ["Good roll", "q-mid"];
      return ["Weak roll", "q-low"];
    }
    function pick3() {
      var pool = POOL.slice(), out = [];
      for (var i = 0; i < 3; i++) out.push(pool.splice(Math.floor(Math.random() * pool.length), 1)[0]);
      return out;
    }
    function renderRoll(names, qs) {
      slotsWrap.innerHTML = "";
      names.forEach(function (n, i) {
        var g = grade(qs[i]);
        var barWrap = el("div", { class: "gg-slot-bar " + g[1] }, el("i"));
        var slot = el("div", { class: "gg-slot" }, [
          el("div", { class: "gg-slot-top" }, [
            el("span", { class: "gg-slot-name", html: '<span class="gg-slot-i"><i class="fa-solid ' + (STAT_ICON[n] || "fa-gem") + '"></i></span>' + tt(n) }),
            el("span", { class: "gg-slot-roll", html: ttf("roll <b>{q}%</b>", { q: qs[i].toFixed(1) }) })
          ]),
          barWrap,
          el("div", { class: "gg-slot-grade", text: tt(g[0]) })
        ]);
        slotsWrap.appendChild(slot);
        (function (bar, q) { requestAnimationFrame(function () { setTimeout(function () { bar.firstChild.style.width = q + "%"; }, 30 + i * 90); }); })(barWrap, qs[i]);
      });
      var avg = qs.reduce(function (a, b) { return a + b; }, 0) / qs.length;
      var g = grade(avg);
      hint.innerHTML = ttf("Average quality <b>{q}%</b> — {grade}. Focusing later lifts every stat to 100%.", { q: avg.toFixed(1), grade: tt(g[0]) });
    }
    function roll() {
      lastNames = pick3();
      lastQs = lastNames.map(function () { return Math.round((55 + Math.random() * 45) * 10) / 10; });
      renderRoll(lastNames, lastQs);
    }
    if (btn) btn.addEventListener("click", roll);
    roll();
    onLang(function () { buildPool(); if (lastNames) renderRoll(lastNames, lastQs); });
  })();

  /* ═══════════════════════════════════════════════════════════════════
     6. Leveling & Power Rank
     ═══════════════════════════════════════════════════════════════════ */
  (function () {
    var pick = $("#gg-level-tierpick"), typePick = $("#gg-level-typepick"),
        slider = $("#gg-level-slider"), chart = $("#gg-pr-chart"),
        valN = $("#gg-level-val"), maxN = $("#gg-level-max"), prN = $("#gg-pr-val"),
        prSub = $("#gg-pr-sub"), primo = $("#gg-primordial"), bonusN = $("#gg-pr-bonus"),
        msWrap = $("#gg-milestones"), hint = $("#gg-level-hint");
    if (!slider) return;
    var tier = TIERS[3], kind = "emp", lastPR = 0;
    function basePR(t, level) { return gemPR(t, kind, level); }   // Lesser or Empowered, per the toggle
    function kindName() { return kind === "emp" ? "Empowered" : "Lesser"; }
    function setPrSub() { if (prSub) prSub.textContent = ttf("perfect {tier} {type} gem", { tier: tt(tier.name), type: tt(kindName()) }); }

    TIERS.forEach(function (t, i) {
      var b = el("button", { type: "button", text: tt(t.name), "aria-checked": i === 3 ? "true" : "false" });
      b.style.setProperty("--tc", t.color);   // each tier button carries its own colour
      if (i === 3) b.classList.add("active");
      b.addEventListener("click", function () { setTier(t, b); });
      pick.appendChild(b);
    });
    if (typePick) [["emp", "Empowered"], ["lesser", "Lesser"]].forEach(function (p) {
      var b = el("button", { type: "button", text: tt(p[1]), "data-kind": p[0], "aria-checked": p[0] === kind ? "true" : "false" });
      if (p[0] === kind) b.classList.add("active");
      b.addEventListener("click", function () { setKind(p[0], b); });
      typePick.appendChild(b);
    });
    function setKind(k, b) {
      kind = k;
      if (typePick) Array.prototype.forEach.call(typePick.children, function (c) { c.classList.remove("active"); c.setAttribute("aria-checked", "false"); });
      b.classList.add("active"); b.setAttribute("aria-checked", "true");
      setPrSub(); buildChart(); update(true);
    }

    function setTier(t, b) {
      tier = t;
      Array.prototype.forEach.call(pick.children, function (c) { c.classList.remove("active"); c.setAttribute("aria-checked", "false"); });
      b.classList.add("active"); b.setAttribute("aria-checked", "true");
      [slider, valN, maxN, prN, msWrap, chart].forEach(function (n) { n.style.setProperty("--gem", t.color); });
      slider.max = t.max; if (+slider.value > t.max) slider.value = t.max;
      maxN.textContent = "/ " + t.max;
      setPrSub();
      buildChart(); buildMs(); update(true);
    }
    function buildMs() {
      msWrap.innerHTML = "";
      [5, 10, 15].forEach(function (lv) {
        var m = el("div", { class: "gg-ms", html: "★<br>" + lv });
        m.style.left = ((lv - 1) / (tier.max - 1) * 100) + "%";
        m.setAttribute("data-lv", lv);
        msWrap.appendChild(m);
      });
    }
    function buildChart() {
      chart.innerHTML = "";
      // Bars = PR GAINED at each level. Milestones add a whole container (worth
      // the tier's high threshold) so they're the tallest jumps by far.
      var gains = [], maxGain = 1, l;
      for (l = 1; l <= tier.max; l++) {
        var g = l === 1 ? 0 : (basePR(tier, l) - basePR(tier, l - 1));
        gains.push(g); if (g > maxGain) maxGain = g;
      }
      for (l = 1; l <= tier.max; l++) {
        var gain = gains[l - 1], milestone = (l === 5 || l === 10 || l === 15);
        var bar = el("div", { class: "gg-bar", "data-lv": l });
        bar.setAttribute("data-h", Math.max(3, gain / maxGain * 100));
        bar.setAttribute("title", ttf(milestone ? "Level {lv}: +{n} PR (new roll!)" : "Level {lv}: +{n} PR", { lv: l, n: gain }));
        if (milestone) bar.classList.add("milestone");
        chart.appendChild(bar);
      }
    }
    function paintChart(level) {
      Array.prototype.forEach.call(chart.children, function (bar) {
        var lv = +bar.getAttribute("data-lv"), h = +bar.getAttribute("data-h");
        bar.style.height = (lv <= level ? h : 3) + "%";
        bar.classList.toggle("on", lv <= level && !bar.classList.contains("milestone"));
        bar.classList.toggle("cursor", lv === level);
      });
    }
    function update(instant) {
      var level = +slider.value, base = basePR(tier, level);
      slider.style.setProperty("--fill", ((level - 1) / (tier.max - 1) * 100) + "%");
      valN.textContent = level;
      countTo(prN, base, { from: instant ? base : lastPR, dur: instant ? 0 : 500 });
      lastPR = base;
      if (bonusN) {
        if (primo && primo.checked) { bonusN.textContent = "+ " + (base * 0.1).toFixed(1) + " (10%)"; bonusN.hidden = false; }
        else { bonusN.hidden = true; }
      }
      Array.prototype.forEach.call(msWrap.children, function (m) { m.classList.toggle("reached", +m.getAttribute("data-lv") <= level); });
      paintChart(level);
      var msg;
      if (level === 1) msg = ttf("A perfect gem starts with its <b>3 stats</b> (3 containers), each worth {n} PR. Drag to pour in Gem Dust.", { n: PR_HI[kind][tier.id] });
      else if (level === 5 || level === 10 || level === 15) msg = ttf("<b>Milestone level {lv}!</b> A new roll (container) drops in - a <b>+{n} PR</b> jump on its own. This is where the big power is.", { lv: level, n: PR_HI[kind][tier.id] });
      else if (level < 15) msg = tt("Steady per-level growth between milestones (×3, one for each stat).");
      else if (level < 20) msg = tt("All <b>6 containers</b> unlocked - the gem is full size. Now it's about focusing every container to 100%.");
      else msg = ttf("Past 15 only static level gains remain; levels divisible by 5 give the largest (+{n} PR here).", { n: tier.pr * 5 * 3 });
      hint.innerHTML = msg;
    }
    slider.addEventListener("input", function () { update(false); });
    if (primo) primo.addEventListener("change", function () { update(true); });
    setTier(TIERS[3], pick.children[3]);
    onLang(function () {
      Array.prototype.forEach.call(pick.children, function (b, i) { b.textContent = tt(TIERS[i].name); });
      if (typePick) Array.prototype.forEach.call(typePick.children, function (b) { b.textContent = tt(b.getAttribute("data-kind") === "emp" ? "Empowered" : "Lesser"); });
      setPrSub(); buildChart(); update(true);
    });
  })();

  /* ═══════════════════════════════════════════════════════════════════
     7. Perfecting with focuses
     ═══════════════════════════════════════════════════════════════════ */
  (function () {
    var ring = $("#gg-focus-ring"), fill = $("#gg-ring-fill"), pctN = $("#gg-ring-pct"),
        countN = $("#gg-focus-count"), btns = Array.prototype.slice.call(doc.querySelectorAll(".gg-focus-btn")),
        resetB = $("#gg-focus-reset"), autoB = $("#gg-focus-auto");
    if (!ring || !fill) return;
    var C = 2 * Math.PI * 52, start = 71, q = start, used = { 1: 0, 2: 0, 3: 0 };

    function render() {
      fill.style.strokeDashoffset = C * (1 - q / 100);
      pctN.innerHTML = (Math.round(q * 10) / 10).toString().replace(/\.0$/, "") + '<span class="gg-ring-unit">%</span>';
      var perfect = q >= 100 - 1e-6;
      ring.classList.toggle("perfect", perfect);
      var total = used[1] + used[2] + used[3];
      if (total === 0) countN.innerHTML = ttf("This stat rolled at <b>{n}%</b>.", { n: start }) + (perfect ? "" : " " + tt("Add focuses to perfect it."));
      else {
        var parts = [];
        [3, 2, 1].forEach(function (k) { if (used[k]) parts.push(used[k] + "× " + AUG[k].name); });
        var list = parts.join(", ");
        countN.innerHTML = perfect
          ? ttf("<b>Perfect!</b> Reached 100% with <b>{list}</b>.", { list: list })
          : ttf("So far: <b>{list}</b> — {n}% to go.", { list: list, n: Math.round((100 - q) * 10) / 10 });
      }
      btns.forEach(function (b) { b.disabled = perfect; });
    }
    function add(k) {
      if (q >= 100) return;
      q = Math.min(100, Math.round((q + AUG[k].val) * 10) / 10);
      used[k]++; render();
    }
    btns.forEach(function (b) { b.addEventListener("click", function () { add(+b.getAttribute("data-aug")); }); });
    if (resetB) resetB.addEventListener("click", function () {
      start = Math.round((58 + Math.random() * 24) * 10) / 10; q = start; used = { 1: 0, 2: 0, 3: 0 }; render();
    });
    if (autoB) autoB.addEventListener("click", function () {
      // greedy: Superior, then Precise, then Rough - fewest focuses that don't overshoot.
      var guard = 0;
      while (q < 100 && guard++ < 60) {
        var gap = 100 - q;
        if (gap >= AUG[3].val) add(3); else if (gap >= AUG[2].val) add(2); else add(1);
      }
    });
    render();
    onLang(render);
  })();

  /* ═══════════════════════════════════════════════════════════════════
     8. Gem Builds - interactive build-code decoder
     ═══════════════════════════════════════════════════════════════════ */
  (function () {
    var modes = $("#gg-build-modes"), viz = $("#gg-build-viz"), detail = $("#gg-build-detail");
    if (!viz || !detail) return;

    // Per gem-group: how many gems and their total boosts (gems × 3 each).
    var GRP = {
      "Empowered":        { gems: 3, sum: 9,  cls: "emp" },
      "Lesser":           { gems: 6, sum: 18, cls: "lesser" },
      "Empowered Cosmic": { gems: 1, sum: 3,  cls: "cosmic" },
      "Lesser Cosmic":    { gems: 2, sum: 6,  cls: "cosmic" }
    };
    function c(group, stat, v) { return { group: group, stat: stat, v: v }; }
    var BUILDS = {
      damage: [c("Empowered", "Damage", 9), c("Empowered", "Critical Damage", 0), c("Lesser", "Damage", 3), c("Lesser", "Critical Damage", 15)],
      farm: [c("Empowered", "Damage", 9), c("Empowered", "Critical Damage", 0), c("Lesser", "Damage", 0), c("Lesser", "Critical Damage", 18),
             c("Empowered Cosmic", "Damage", 3), c("Empowered Cosmic", "Critical Damage", 0), c("Empowered Cosmic", "Light", 0),
             c("Lesser Cosmic", "Damage", 1), c("Lesser Cosmic", "Critical Damage", 1), c("Lesser Cosmic", "Light", 4)]
    };
    var MODES = [["damage", "Damage build"], ["farm", "Farm build"]];
    var mode = "damage";

    function shortStat(s) { return s === "Critical Damage" ? tt("Crit") : (s === "Damage" ? tt("Damage") : tt("Light")); }
    function hi(g) {
      Array.prototype.forEach.call(viz.querySelectorAll(".gg-code-grp"), function (cl) {
        cl.classList.toggle("hot", g && cl.getAttribute("data-group") === g);
        cl.classList.toggle("dim", g && cl.getAttribute("data-group") !== g);
      });
    }
    function defaultDetail() {
      detail.className = "gg-build-detail";
      detail.innerHTML = '<p class="gg-bd-hint"><i class="fa-solid fa-hand-pointer" aria-hidden="true"></i> ' + tt("Hover or tap any number to see what it means.") + "</p>";
    }
    function showCell(cell) {
      var g = GRP[cell.group];
      detail.className = "gg-build-detail active " + g.cls;
      detail.innerHTML =
        '<div class="gg-bd-top"><span class="gg-bd-num">' + cell.v + '</span><span class="gg-bd-lbl">' + (cell.v === 1 ? tt("boost") : tt("boosts")) + "</span></div>" +
        '<div class="gg-bd-path"><b>' + tt(cell.group) + '</b> <i class="fa-solid fa-arrow-right" aria-hidden="true"></i> <b>' + tt(cell.stat) + "</b></div>" +
        "<p>" + ttf("Your {group} gems drop {sum} boosts in total ({gems} × 3). This is how many of them land on {stat}.", { group: tt(cell.group), sum: g.sum, gems: g.gems, stat: tt(cell.stat) }) + "</p>";
    }

    function render() {
      if (modes) {
        modes.innerHTML = "";
        MODES.forEach(function (m) {
          var b = el("button", { type: "button", class: "gg-seg" + (m[0] === mode ? " active" : ""), role: "radio", "aria-checked": m[0] === mode ? "true" : "false", text: tt(m[1]) });
          b.addEventListener("click", function () { mode = m[0]; render(); });
          modes.appendChild(b);
        });
      }
      viz.innerHTML = "";
      var cells = BUILDS[mode], order = [], byGroup = {};
      cells.forEach(function (cell) { if (!byGroup[cell.group]) { byGroup[cell.group] = []; order.push(cell.group); } byGroup[cell.group].push(cell); });
      var row = el("div", { class: "gg-code-row" });
      order.forEach(function (gName, gi) {
        if (gi > 0) {
          // Same-realm groups (Empowered↔Lesser) are separated by a plain space -
          // the row gap - for clarity: "9/0 3/15", not "9/0/3/15". Only the jump
          // from elemental to Cosmic keeps a visible "+".
          var isCos = gName.indexOf("Cosmic") >= 0, prevCos = order[gi - 1].indexOf("Cosmic") >= 0;
          if (isCos && !prevCos) row.appendChild(el("span", { class: "gg-code-sep plus", text: "+" }));
        }
        var g = GRP[gName];
        var cluster = el("div", { class: "gg-code-grp " + g.cls, "data-group": gName }, el("span", { class: "gg-code-grplabel", text: tt(gName) }));
        var cw = el("div", { class: "gg-code-cells" });
        byGroup[gName].forEach(function (cell, ci) {
          if (ci > 0) cw.appendChild(el("span", { class: "gg-code-sep sm", text: "/" }));
          var btn = el("button", { type: "button", class: "gg-code-cell", text: String(cell.v), "aria-label": cell.v + " boosts, " + gName + " " + cell.stat });
          var slot = el("span", { class: "gg-code-slot" }, [btn, el("span", { class: "gg-code-stat", text: shortStat(cell.stat) })]);
          (function (cc) {
            function on() { hi(cc.group); showCell(cc); }
            function off() { hi(null); defaultDetail(); }
            btn.addEventListener("mouseenter", on); btn.addEventListener("mouseleave", off);
            btn.addEventListener("focus", on); btn.addEventListener("blur", off);
          })(cell);
          cw.appendChild(slot);
        });
        cluster.appendChild(cw);
        row.appendChild(cluster);
      });
      viz.appendChild(row);
      defaultDetail();
    }
    render();
    onLang(render);
  })();

  /* ═══════════════════════════════════════════════════════════════════
     9. Advanced - generated stat tables (mirrors Gem.stat_values)
     ═══════════════════════════════════════════════════════════════════ */
  (function () {
    var controls = $("#gg-tbl-controls"), table = $("#gg-stat-table");
    if (!controls || !table) return;
    var st = { tier: TIERS[1], kind: "lesser", boosts: 0 };   // default Stellar Lesser (matches the reference sheet)

    function activate(box, btn) {
      Array.prototype.forEach.call(box.children, function (c) { c.classList.remove("active"); c.setAttribute("aria-checked", "false"); });
      btn.classList.add("active"); btn.setAttribute("aria-checked", "true");
    }
    function segGroup(label, items, isActive, pick, colorFn) {
      var seg = el("div", { class: "gg-seg-pick", role: "radiogroup", "aria-label": label });
      items.forEach(function (it) {
        var b = el("button", { type: "button", text: it.label, "aria-checked": isActive(it) ? "true" : "false" });
        if (colorFn) b.style.setProperty("--tc", colorFn(it));
        if (isActive(it)) b.classList.add("active");
        b.addEventListener("click", function () { activate(seg, b); pick(it); render(); });
        seg.appendChild(b);
      });
      return el("div", { class: "gg-tbl-cgroup" }, [el("span", { class: "gg-tbl-clabel", text: label }), seg]);
    }
    function buildControls() {
      controls.innerHTML = "";
      controls.appendChild(segGroup(tt("Tier"),
        TIERS.map(function (t) { return { label: tt(t.name), t: t }; }),
        function (it) { return it.t.id === st.tier.id; }, function (it) { st.tier = it.t; },
        function (it) { return it.t.color; }));
      controls.appendChild(segGroup(tt("Type"),
        [{ label: tt("Lesser"), k: "lesser" }, { label: tt("Empowered"), k: "emp" }],
        function (it) { return it.k === st.kind; }, function (it) { st.kind = it.k; }));
      controls.appendChild(segGroup(tt("Boosts on this stat"),
        [0, 1, 2, 3].map(function (n) { return { label: String(n), b: n }; }),
        function (it) { return it.b === st.boosts; }, function (it) { st.boosts = it.b; }));
    }
    function render() {
      table.style.setProperty("--gem", st.tier.color);
      var startLvl = Math.max(1, 5 * st.boosts), lv, i;
      var h = "<thead><tr><th>" + tt("Level") + "</th><th>" + tt("PR/lvl") + "</th>";
      for (i = 0; i < STAT_COLS.length; i++) h += "<th>" + STAT_COLS[i].label + "</th>";
      h += "</tr></thead><tbody>";
      for (lv = startLvl; lv <= st.tier.max; lv++) {
        var milestone = (lv === 5 || lv === 10 || lv === 15 || (lv > 15 && lv % 5 === 0));
        h += "<tr" + (milestone ? ' class="ms"' : "") + '><th>' + lv + '</th><td class="prlvl">' + prInc(lv, st.tier.pr) + "</td>";
        for (i = 0; i < STAT_COLS.length; i++) {
          var c = STAT_COLS[i], v = statValue(st.tier, st.kind, c, lv, st.boosts);
          h += '<td><span class="mn">' + v[0].toFixed(c.dec) + '</span><span class="mx">' + v[1].toFixed(c.dec) + "</span></td>";
        }
        h += "</tr>";
      }
      table.innerHTML = h + "</tbody>";
    }
    buildControls(); render();
    onLang(function () { buildControls(); render(); });
  })();

  /* ═══════════════════════════════════════════════════════════════════
     Reveal-on-scroll + section-nav + count-up-on-view
     ═══════════════════════════════════════════════════════════════════ */
  (function () {
    body.classList.add("gg-js");
    var reveals = Array.prototype.slice.call(doc.querySelectorAll("[data-reveal]"));
    // Stagger index within each reveal's parent.
    reveals.forEach(function (n) {
      var sibs = Array.prototype.filter.call(n.parentNode.children, function (c) { return c.hasAttribute && c.hasAttribute("data-reveal"); });
      n.style.setProperty("--i", sibs.indexOf(n));
    });

    var vh = window.innerHeight || doc.documentElement.clientHeight;
    function fireCounts(scope) {
      Array.prototype.slice.call(scope.querySelectorAll("[data-count]")).forEach(function (c) {
        if (c.getAttribute("data-done")) return;
        c.setAttribute("data-done", "1");
        countTo(c, +c.getAttribute("data-count"), { suffix: c.getAttribute("data-suffix") || "", dur: 900 });
      });
    }

    if (REDUCE) {
      reveals.forEach(function (n) { n.classList.add("in"); });
      fireCounts(doc);
    } else {
      // Scroll-driven reveal: any target whose top has scrolled into ~92% of the
      // viewport reveals once, then is skipped. Runs on load + throttled scroll,
      // so nothing ships blank in a headless renderer (unlike IO gated purely on
      // scroll callbacks).
      var pending = reveals.slice();
      function sweep() {
        var h = window.innerHeight || vh;
        pending = pending.filter(function (n) {
          var r = n.getBoundingClientRect();
          if (r.top < h * 0.92 && r.bottom > 0) { n.classList.add("in"); fireCounts(n); return false; }
          return true;
        });
        if (!pending.length) { window.removeEventListener("scroll", sweep); window.removeEventListener("resize", sweep); }
      }
      window.addEventListener("scroll", sweep, { passive: true });
      window.addEventListener("resize", sweep);
      // 'load' fires after the browser applies any #hash deep-link scroll, so a
      // section linked directly (e.g. /gems-guide#focus) still reveals.
      window.addEventListener("load", sweep);
      sweep();
    }

    // Section-nav active state.
    var secnav = $("#gg-secnav");
    if (secnav && "IntersectionObserver" in window) {
      var links = Array.prototype.slice.call(secnav.querySelectorAll("a"));
      var map = {};
      links.forEach(function (a) { map[a.getAttribute("href").slice(1)] = a; });
      var sections = links.map(function (a) { return doc.getElementById(a.getAttribute("href").slice(1)); }).filter(Boolean);
      var navio = new IntersectionObserver(function (entries) {
        entries.forEach(function (e) {
          if (e.isIntersecting) {
            links.forEach(function (l) { l.classList.remove("active"); });
            if (map[e.target.id]) map[e.target.id].classList.add("active");
          }
        });
      }, { rootMargin: "-45% 0px -50% 0px", threshold: 0 });
      sections.forEach(function (s) { navio.observe(s); });
    }
  })();

  /* ═══════════════════════════════════════════════════════════════════
     View toggle: Guide (story, scroll-driven) ↔ Wiki (compact, tabbed,
     info-first). Wiki mode drops the flavor, hides the narrative leads,
     and shows one section at a time - the secnav acts as tabs. Persisted.
     ═══════════════════════════════════════════════════════════════════ */
  (function () {
    var secnav = $("#gg-secnav");
    if (!secnav) return;
    var KEY = "gg-view";
    // Precedence: ?view= in the URL (so a shared link forces its view) >
    // saved preference > default (wiki). Toggling writes both back to the URL.
    function readView() {
      var q;
      try { q = new URLSearchParams(location.search).get("view"); } catch (e) {}
      if (q === "wiki" || q === "guide") return q;
      try { var s = localStorage.getItem(KEY); if (s === "wiki" || s === "guide") return s; } catch (e) {}
      return "wiki";
    }
    function writeUrl(v, tab) {
      try {
        var u = new URL(location.href);
        u.searchParams.set("view", v);
        if (v === "wiki" && tab) u.hash = tab;
        history.replaceState(null, "", u.pathname + u.search + u.hash);
      } catch (e) {}
    }
    var view = readView();

    var lbl = el("span", { class: "gg-view-toggle-lbl", text: tt("View") });
    var bGuide = el("button", { type: "button", role: "radio", text: tt("Guide") });
    var bWiki = el("button", { type: "button", role: "radio", text: tt("Wiki") });
    var seg = el("div", { class: "gg-view-seg", role: "radiogroup", "aria-label": tt("Page view") }, [bGuide, bWiki]);
    var bar = el("div", { class: "gg-view-toggle" }, [lbl, seg]);
    secnav.parentNode.insertBefore(bar, secnav);

    var links = Array.prototype.slice.call(secnav.querySelectorAll("a"));
    var sections = links.map(function (a) { return doc.getElementById(a.getAttribute("href").slice(1)); });
    var curTab = null;

    function showTab(id, scroll) {
      curTab = id;
      sections.forEach(function (s, i) {
        if (!s) return;
        var on = s.id === id;
        s.classList.toggle("gg-wiki-active", on);
        links[i].classList.toggle("active", on);
      });
      if (scroll) { var t = doc.getElementById(id); if (t) t.scrollIntoView({ block: "start" }); }
      if (view === "wiki") writeUrl("wiki", id);
    }
    function firstTab() {
      var h = (location.hash || "").slice(1);
      var ids = sections.filter(Boolean).map(function (s) { return s.id; });
      return ids.indexOf(h) >= 0 ? h : (ids[0] || null);
    }
    function apply(v, scroll) {
      view = v;
      body.classList.toggle("gg-wiki", v === "wiki");
      bGuide.classList.toggle("active", v === "guide");
      bWiki.classList.toggle("active", v === "wiki");
      bGuide.setAttribute("aria-checked", v === "guide" ? "true" : "false");
      bWiki.setAttribute("aria-checked", v === "wiki" ? "true" : "false");
      if (v === "wiki") showTab(curTab || firstTab(), scroll);
      else writeUrl("guide", null);
    }
    function remember(v) { try { localStorage.setItem(KEY, v); } catch (e) {} }

    links.forEach(function (a) {
      a.addEventListener("click", function (ev) {
        if (view !== "wiki") return;      // guide mode: normal anchor scroll
        ev.preventDefault();
        showTab(a.getAttribute("href").slice(1), true);
      });
    });
    bGuide.addEventListener("click", function () { apply("guide", false); remember("guide"); });
    bWiki.addEventListener("click", function () { apply("wiki", true); remember("wiki"); });

    apply(view, false);
    onLang(function () {
      lbl.textContent = tt("View");
      bGuide.textContent = tt("Guide"); bWiki.textContent = tt("Wiki");
      seg.setAttribute("aria-label", tt("Page view"));
    });
  })();

})();

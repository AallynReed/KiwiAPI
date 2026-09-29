/* ===========================================================================
   home_voxel.js - the Voxel Island homepage (/).

   The hero is a voxel island drawn on <canvas>, built from the biomes that are
   in rotation right now (each biome's `icon` key from the game data picks its
   look) and lit for Trove server time. Merchants stand where they are.
   Everything else on the page is server-rendered first (templates/home_voxel.html)
   and refreshed here from the same sources the classic dashboard uses:
     rotations -> /site/rotations           records  -> /site/leaderboards/records
     servers   -> /site/trove-status        players  -> /site/leaderboards/activity
     update    -> /site/updates/live-us/versions        giveaway -> /site/giveaways
   Every section fails closed: an error leaves the server copy in place.
   ========================================================================== */
(function () {
  "use strict";

  var U = window.BTTUtil || {};
  var getJSON = U.getJSON || function (u) { return fetch(u).then(function (r) { return r.ok ? r.json() : Promise.reject(r.status); }); };
  function tr(s) { return window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s; }
  function esc(s) { return U.esc ? U.esc(s) : String(s == null ? "" : s).replace(/[&<>"']/g, function (c) { return "&#" + c.charCodeAt(0) + ";"; }); }
  function $(s, r) { return (r || document).querySelector(s); }
  function $$(s, r) { return Array.prototype.slice.call((r || document).querySelectorAll(s)); }
  function pad(n) { return String(n).padStart(2, "0"); }
  function num(n) { return n == null ? "—" : Number(n).toLocaleString(); }

  var HOUR = 3600e3, DAY = 86400e3;
  var root = document.documentElement;
  var reduce = window.matchMedia("(prefers-reduced-motion: reduce)");
  var clockOffset = 0;                                   // server clock - local clock, ms
  function now() { return Date.now() + clockOffset; }
  function server(t) { return new Date((t === undefined ? now() : t) - 11 * HOUR); }
  function todFor(h) { return h >= 5 && h < 9 ? "morning" : h >= 9 && h < 17 ? "day" : h >= 17 && h < 20 ? "dusk" : "night"; }
  function fmt(ms) {
    if (ms == null || isNaN(ms)) return "—";
    if (ms <= 0) return tr("now");
    var ts = Math.ceil(ms / 1000);
    if (ts >= 86400) { var th = Math.ceil(ts / 3600); return Math.floor(th / 24) + "d " + (th % 24) + "h"; }
    if (ts >= 3600) { var tm = Math.ceil(ts / 60); return Math.floor(tm / 60) + "h " + (tm % 60) + "m"; }
    return Math.floor(ts / 60) + "m " + pad(ts % 60) + "s";
  }

  /* ── Colour helpers ─────────────────────────────────────────────────── */
  function hex(h) { var n = parseInt(h.slice(1), 16); return [n >> 16 & 255, n >> 8 & 255, n & 255]; }
  function mix(a, b, t) { return [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]; }
  function css(c) { return "rgb(" + Math.round(c[0]) + "," + Math.round(c[1]) + "," + Math.round(c[2]) + ")"; }
  function mulberry32(a) {
    return function () {
      a |= 0; a = a + 0x6D2B79F5 | 0;
      var t = Math.imul(a ^ a >>> 15, 1 | a);
      t = t + Math.imul(t ^ t >>> 7, 61 | t) ^ t;
      return ((t ^ t >>> 14) >>> 0) / 4294967296;
    };
  }
  function hashStr(s) { var h = 0x811c9dc5; for (var i = 0; i < s.length; i++) { h ^= s.charCodeAt(i); h = Math.imul(h, 16777619); } return h >>> 0; }

  /* ── Props: small builds out of boxes [x, y, z, w, d, h] in tile units ── */
  var LEFT = .8, RIGHT = .63;
  function B(x, y, z, w, d, h, c, top, faces, glow) {
    return { x: x, y: y, z: z, w: w, d: d, h: h, c: hex(c), top: top ? hex(top) : null, faces: faces || "tlr", glow: glow ? hex(glow) : null };
  }
  var P = {
    pine: function (x, y, h) { var px = x + .5, py = y + .5; return [
      B(px - .08, py - .08, h, .16, .16, .35, "#6b5446"),
      B(px - .4, py - .4, h + .3, .8, .8, .35, "#3f6d62", "#e6eef3"),
      B(px - .3, py - .3, h + .65, .6, .6, .35, "#437265", "#edf2f6"),
      B(px - .19, py - .19, h + 1, .38, .38, .32, "#477769", "#f4f7fa"),
      B(px - .08, py - .08, h + 1.32, .16, .16, .18, "#f4f7fa")]; },
    faeTree: function (x, y, h, r) { var px = x + .5, py = y + .5, s = .74 + r * .18; return [
      B(px - .1, py - .1, h, .2, .2, .55, "#8a6c58"),
      B(px - s / 2, py - s / 2, h + .5, s, s, .62, "#5fa35f", "#78bb70"),
      B(px - s / 3.2, py - s / 3.2, h + 1.12, s / 1.6, s / 1.6, .34, "#67ad65", "#86c67b")]; },
    mushroom: function (x, y, h) { var px = x + .5, py = y + .5; return [
      B(px - .08, py - .08, h, .16, .16, .32, "#f1e6d4"),
      B(px - .26, py - .26, h + .32, .52, .52, .16, "#d9655a", "#e67a6b")]; },
    deadTree: function (x, y, h) { var px = x + .5, py = y + .5, k = "#5d5064"; return [
      B(px - .3, py - .05, h + .55, .23, .1, .1, k),
      B(px - .07, py - .07, h, .14, .14, 1.25, k),
      B(px + .07, py - .05, h + .72, .3, .1, .1, k),
      B(px - .05, py + .07, h + .95, .1, .26, .1, k)]; },
    grave: function (x, y, h) { var px = x + .5, py = y + .5; return [B(px - .2, py - .07, h, .4, .14, .5, "#b7afc0", "#c8c1cf")]; },
    lolly: function (x, y, h, r) { var px = x + .5, py = y + .5, c = r < .4 ? "#ef8fb3" : r < .75 ? "#9fd8c6" : "#f5d68a"; return [
      B(px - .05, py - .05, h, .1, .1, 1, "#fbf3ea"),
      B(px - .24, py - .24, h + 1, .48, .48, .48, c)]; },
    gumdrop: function (x, y, h, r) { var px = x + .5, py = y + .5, c = r < .5 ? "#f6b8cf" : "#b6e2d3"; return [
      B(px - .25, py - .25, h, .5, .5, .3, c),
      B(px - .16, py - .16, h + .3, .32, .32, .18, c)]; },
    palm: function (x, y, h, r, a, b) { var px = x + .5, py = y + .5, l = (r - .5) * .3; return [
      B(px - .07, py - .07, h, .14, .14, .5, "#8a6a4c"),
      B(px - .07 + l * .5, py - .07, h + .5, .14, .14, .5, "#8a6a4c"),
      B(px - .07 + l, py - .07, h + 1, .14, .14, .42, "#8a6a4c"),
      B(px - .5 + l, py - .12, h + 1.38, 1, .24, .12, a, b),
      B(px - .12 + l, py - .5, h + 1.38, .24, 1, .12, a, b),
      B(px - .15 + l, py - .15, h + 1.46, .3, .3, .14, b)]; },
    fern: function (x, y, h) { var px = x + .5, py = y + .5; return [B(px - .3, py - .3, h, .6, .6, .25, "#4f8f45", "#63a653")]; },
    lava: function (x, y, h) { return [B(x + .12, y + .12, h, .76, .76, .06, "#c9542a", "#f07a3a", null, "#ffb35a")]; },
    spike: function (x, y, h) { var px = x + .5, py = y + .5; return [
      B(px - .14, py - .14, h, .28, .28, 1.1, "#3a2c2b", "#4d3a38"),
      B(px - .08, py - .08, h + 1.1, .16, .16, .4, "#3a2c2b")]; },
    crystal: function (x, y, h, r) { var px = x + .5, py = y + .5, c = r < .5 ? "#7fd3d0" : "#b59be8", g = r < .5 ? "#b8f2ee" : "#d9ccff"; return [
      B(px - .12, py - .12, h, .24, .24, .9, c, null, null, g),
      B(px + .1, py - .05, h, .16, .16, .55, c, null, null, g)]; },
    oak: function (x, y, h, r, s) { var px = x + .5, py = y + .5; return [
      B(px - .1 * s, py - .1 * s, h, .2 * s, .2 * s, .6 * s, "#7a5a42"),
      B(px - .42 * s, py - .42 * s, h + .6 * s, .84 * s, .84 * s, .6 * s, "#4f8f45", "#66a857"),
      B(px - .28 * s, py - .28 * s, h + 1.2 * s, .56 * s, .56 * s, .3 * s, "#5a9a4d", "#72b561")]; },
    cactus: function (x, y, h) { var px = x + .5, py = y + .5, g = "#5f9e56"; return [
      B(px - .3, py - .06, h + .3, .2, .12, .12, g),
      B(px - .3, py - .06, h + .3, .1, .12, .34, g),
      B(px - .1, py - .1, h, .2, .2, 1, g, "#76b86a"),
      B(px + .1, py - .06, h + .45, .2, .12, .12, g),
      B(px + .2, py - .06, h + .45, .1, .12, .36, g)]; },
    rock: function (x, y, h, c) { var px = x + .5, py = y + .5; return [
      B(px - .25, py - .2, h, .5, .4, .3, c),
      B(px - .1, py - .05, h + .3, .25, .2, .15, c)]; },
    shrub: function (x, y, h) { var px = x + .5, py = y + .5; return [B(px - .2, py - .2, h, .4, .4, .2, "#8a8060", "#9c9270")]; },
    spire: function (x, y, h, r) { var px = x + .5, py = y + .5, H = 1.6 + r * 2; return [
      B(px - .2, py - .2, h, .4, .4, H, "#7f6db8", "#9886cf"),
      B(px - .12, py - .12, h + H, .24, .24, .35, "#8fe0d0", null, null, "#c2f7ec")]; },
    lantern: function (x, y, h) { var px = x + .5, py = y + .5; return [
      B(px - .06, py - .06, h, .12, .12, 1, "#76604f"),
      B(px - .15, py - .15, h + 1, .3, .3, .3, "#f0dcb0", null, null, "#ffc964"),
      B(px - .18, py - .18, h + 1.3, .36, .36, .06, "#5a4a3f")]; },
  };
  function tower(x, y, h, H, prand) {
    var boxes = [B(x + .1, y + .1, h, .8, .8, H, "#3b4059", "#5d6486")];
    if (H >= 4) boxes.push(B(x + .46, y + .46, h + H, .08, .08, .7, "#5d6486"));
    var win = [];
    for (var k = 0; k < H; k++) for (var j = 0; j < 2; j++) {
      var a = .1 + .8 * (.16 + j * .42), b = a + .8 * .26, z0 = h + k + .3, z1 = z0 + .4;
      win.push({ f: RIGHT, lit: prand() < .62, lamp: hex(prand() < .8 ? "#ffd07a" : "#a6e4da"), q: [[x + .9, y + a, z0], [x + .9, y + b, z0], [x + .9, y + b, z1], [x + .9, y + a, z1]] });
      win.push({ f: LEFT, lit: prand() < .62, lamp: hex(prand() < .8 ? "#ffd07a" : "#a6e4da"), q: [[x + a, y + .9, z0], [x + b, y + .9, z0], [x + b, y + .9, z1], [x + a, y + .9, z1]] });
    }
    boxes.win = win;
    return boxes;
  }

  /* ── Biome looks, keyed by the game's biome icon ─────────────────────── */
  var STYLE = {
    hub: { top: ["#d9d0c1", "#d3c9b9"], edge: "#bfb1a0", under: "#ab9d8c", beach: false, marker: "#d9a86c" },
    candy: { top: ["#f2a8c4", "#f5b6ce", "#fbeee6"], edge: "#f8e2d4", under: "#e999b8", layer: ["#f8e2d4", "#e999b8"], marker: "#ec94b5",
      h: function (f) { return 2 + Math.round(f * 2.2); },
      prop: function (x, y, h, r, r2) { return r < .13 ? P.lolly(x, y, h, r2) : r < .23 ? P.gumdrop(x, y, h, r2) : null; } },
    neon: { top: ["#4f5572", "#585f7e", "#4a506b"], edge: "#464c67", under: "#3b4058", beach: false, tall: true, marker: "#6f78a3",
      h: function (f) { return 2 + (f > .58 ? 1 : 0); },
      prop: function (x, y, h, r, r2, pr) { return r < .45 ? tower(x, y, h, 2 + Math.floor(r2 * 4.2), pr) : null; } },
    tundra: { top: ["#f3f6f9", "#e9f0f5"], edge: "#e2ebf1", under: "#a5cee2", beach: false, tall: true, cap: 2.4, marker: "#9ccbe2",
      h: function (f) { return 2 + Math.round(Math.pow(f, 1.2) * 6.5); },
      prop: function (x, y, h, r) { return r < .42 ? P.pine(x, y, h) : null; } },
    undead: { top: ["#9d92ab", "#a69cb3", "#968aa4"], edge: "#877b98", under: "#766a86", marker: "#9a8fa8",
      h: function (f) { return 2 + Math.round(f * 3.4); },
      prop: function (x, y, h, r) { return r < .32 ? P.deadTree(x, y, h) : r < .46 ? P.grave(x, y, h) : null; } },
    fae: { top: ["#8cc680", "#80bd77", "#97cf89"], edge: "#72ad6c", under: "#b39b83", marker: "#6fb46f",
      h: function (f) { return 1 + Math.round(f * 2.6); },
      prop: function (x, y, h, r, r2) { return r < .26 ? P.faeTree(x, y, h, r2) : r < .33 ? P.mushroom(x, y, h) : null; } },
    dinosaur: { top: ["#5f9e4f", "#6aa857", "#579447"], edge: "#4f8a42", under: "#8a6a4c", marker: "#6aa857",
      h: function (f) { return 1 + Math.round(f * 3); },
      prop: function (x, y, h, r, r2) { return r < .3 ? P.palm(x, y, h, r2, "#4f9a47", "#6db85a") : r < .36 ? P.fern(x, y, h) : null; } },
    dragon: { top: ["#5a4644", "#634d4a", "#523f3d"], edge: "#4d3b39", under: "#3f302f", beach: false, tall: true, cap: 2, marker: "#d0643e",
      h: function (f) { return 2 + Math.round(Math.pow(f, 1.3) * 6); },
      prop: function (x, y, h, r) { return r < .12 ? P.lava(x, y, h) : r < .22 ? P.spike(x, y, h) : null; } },
    dunes: { top: ["#d9c29a", "#e2cca4", "#cfb78e"], edge: "#c4aa80", under: "#a88f6a", marker: "#7fd3d0",
      h: function (f) { return 1 + Math.round(f * 2.4); },
      prop: function (x, y, h, r, r2) { return r < .14 ? P.crystal(x, y, h, r2) : null; } },
    forest: { top: ["#7db86a", "#88c273", "#74ad62"], edge: "#6aa159", under: "#9b7f60", marker: "#7db86a",
      h: function (f) { return 1 + Math.round(f * 3); },
      prop: function (x, y, h, r, r2) { return r < .24 ? P.oak(x, y, h, r2, 1) : null; } },
    frontier: { top: ["#e8b877", "#edc387", "#e1ad6a"], edge: "#d49c5c", under: "#c0864b", marker: "#e8a857",
      h: function (f) { return 1 + Math.round(f * 1.6) + (f > .7 ? 3 : 0); },
      prop: function (x, y, h, r) { return r < .14 ? P.cactus(x, y, h) : null; } },
    giantland: { top: ["#9ccf6e", "#a8d77a", "#92c566"], edge: "#86b85c", under: "#8f7a5a", marker: "#9ccf6e",
      h: function (f) { return 2 + Math.round(f * 3.2); },
      prop: function (x, y, h, r, r2) { return r < .14 ? P.oak(x, y, h, r2, 1.35) : null; } },
    pirate: { top: ["#8ccf8a", "#98d694"], edge: "#7cbf7a", under: "#c9b18a", marker: "#f0cf8a",
      h: function (f) { return 1 + Math.round(f * 1.5); },
      prop: function (x, y, h, r, r2) { return r < .2 ? P.palm(x, y, h, r2, "#4fa257", "#6fc070") : null; } },
    sandsea: { top: ["#e3a08c", "#e9ad98", "#dc9480"], edge: "#d18a77", under: "#b8735f", marker: "#e3a08c",
      h: function (f) { return 1 + Math.round(f * 2.2); },
      prop: function (x, y, h, r) { return r < .06 ? P.rock(x, y, h, "#a6614f") : null; } },
    spires: { top: ["#7f86a8", "#8990b2", "#767d9e"], edge: "#6d7394", under: "#5b6080", tall: true, marker: "#8d78c4",
      h: function (f) { return 2 + Math.round(f * 2.5); },
      prop: function (x, y, h, r, r2) { return r < .2 ? P.spire(x, y, h, r2) : null; } },
    wasteland: { top: ["#a39a88", "#aba290", "#9b927f"], edge: "#8f8674", under: "#7d7462", marker: "#a39a88",
      h: function (f) { return 1 + Math.round(f * 2.4); },
      prop: function (x, y, h, r) { return r < .1 ? P.rock(x, y, h, "#857b6a") : r < .16 ? P.shrub(x, y, h) : null; } },
  };
  Object.keys(STYLE).forEach(function (k) {
    var s = STYLE[k];
    s.top = s.top.map(hex); s.edge = hex(s.edge); s.under = hex(s.under);
    if (s.layer) s.layer = s.layer.map(hex);
  });
  var MAT = {
    deep: hex("#4a8fb8"), shallow: hex("#70b8c8"), foam: hex("#9ed5d3"), waterSide: hex("#3f80a8"),
    sand: hex("#eedcb4"), sandSide: hex("#dcc398"), path: hex("#e4be8b"),
    strata: [hex("#b39d86"), hex("#c7b194"), hex("#dcc8a2")], glass: hex("#6f7a9c"),
    moon: [hex("#eef1f8"), hex("#cdd5e6"), hex("#aab5cf")], crater: hex("#d3d9e7"),
  };
  var RAW_ENV = {
    morning: { sky: ["#9cc0e2", "#c4d4e7", "#f2d6c8", "#f9e3d2"], light: [1.03, .97, .93], tint: [255, 190, 160], tintAmt: .07, stars: 0, lamps: .12, sunA: 1, moonA: 0, sunX: .05, sunY: .3, sunCol: ["#fff3d8", "#ffd9a2", "#f4bd7e"], cloud: "#fff8f2", cloudShade: "#efd6cf", cloudA: .95 },
    day: { sky: ["#69addd", "#8fc3e6", "#c2e0ef", "#dcedf5"], light: [1, 1, 1], tint: [255, 255, 255], tintAmt: 0, stars: 0, lamps: 0, sunA: 1, moonA: 0, sunX: .86, sunY: .06, sunCol: ["#fffbe2", "#ffe8a6", "#fbd57a"], cloud: "#ffffff", cloudShade: "#dbe8f1", cloudA: 1 },
    dusk: { sky: ["#29234d", "#4e3565", "#9a5670", "#eea06c"], light: [.97, .79, .74], tint: [255, 130, 100], tintAmt: .1, stars: .3, lamps: .7, sunA: 1, moonA: 0, sunX: .96, sunY: .43, sunCol: ["#ffd8a6", "#f7a66c", "#e0845a"], cloud: "#8e5876", cloudShade: "#6c4466", cloudA: .9 },
    night: { sky: ["#060a1a", "#0d1430", "#182247", "#263158"], light: [.38, .45, .64], tint: [30, 50, 110], tintAmt: .08, stars: 1, lamps: 1, sunA: 0, moonA: 1, sunX: .97, sunY: .8, sunCol: ["#ffd8a6", "#f7a66c", "#e0845a"], cloud: "#34406a", cloudShade: "#26304f", cloudA: .75 },
  };
  var ENV = {};
  Object.keys(RAW_ENV).forEach(function (k) {
    var e = Object.assign({}, RAW_ENV[k]);
    e.sky = e.sky.map(hex); e.sunCol = e.sunCol.map(hex); e.cloud = hex(e.cloud); e.cloudShade = hex(e.cloudShade);
    ENV[k] = e;
  });
  function lerpEnv(a, b, t) {
    var o = {};
    Object.keys(b).forEach(function (k) {
      var va = a[k], vb = b[k];
      if (Array.isArray(vb)) o[k] = Array.isArray(vb[0]) ? vb.map(function (c, i) { return mix(va[i], c, t); }) : mix(va, vb, t);
      else o[k] = va + (vb - va) * t;
    });
    return o;
  }

  /* ── Island generation (pure) ────────────────────────────────────────── */
  function genWorld(regions, seed) {
    var rand = mulberry32(seed), prand = mulberry32(seed ^ 0x5f3759df);
    function noise(size) {
      var g = new Float32Array(size * size);
      for (var i = 0; i < g.length; i++) g[i] = rand();
      function at(i, j) { return g[((j % size + size) % size) * size + ((i % size + size) % size)]; }
      return function (x, y) {
        var xi = Math.floor(x), yi = Math.floor(y), u = x - xi, v = y - yi;
        u = u * u * (3 - 2 * u); v = v * v * (3 - 2 * v);
        var a = at(xi, yi), b = at(xi + 1, yi), c = at(xi, yi + 1), d = at(xi + 1, yi + 1);
        return a + (b - a) * u + (c - a) * v + (a - b - c + d) * u * v;
      };
    }
    var n1 = noise(16), n2 = noise(16), n3 = noise(16);
    function fbm(x, y) { return n1(x / 5, y / 5) * .6 + n2(x / 2.6, y / 2.6) * .3 + n3(x / 1.3, y / 1.3) * .1; }
    var N = 28, WL = .62, FLOOR = -3, CEN = (N - 1) / 2, RAD = N / 2 - 1.9;
    var K = regions.length, SPAN = 360 / K, START = -165;
    var cols = [];
    function at(x, y) { return (x < 0 || y < 0 || x >= N || y >= N) ? null : cols[x * N + y]; }
    function regionAt(deg) { var a = ((deg - START) % 360 + 360) % 360; return regions[Math.min(K - 1, Math.floor(a / SPAN))]; }

    for (var x = 0; x < N; x++) for (var y = 0; y < N; y++) {
      var dx = x - CEN, dy = y - CEN;
      var r = Math.pow(Math.pow(Math.abs(dx), 2.6) + Math.pow(Math.abs(dy), 2.6), 1 / 2.6) / RAD;
      var border = Math.min(x, y, N - 1 - x, N - 1 - y);
      var ang = Math.atan2(dx + dy, dx - dy);
      var edge = .9 + (n1(Math.cos(ang) * 1.7 + 8, Math.sin(ang) * 1.7 + 8) - .5) * .3 + (fbm(x, y) - .5) * .1;
      var coast = Math.min(edge - r, (border - 1.6) / RAD);
      var c = { x: x, y: y, h: 0, water: false, biome: "sea", props: null, propTop: 0, top: null, beach: false, path: false, foam: false, jit: .97 + rand() * .06 };
      if (coast <= 0) {
        c.water = true; c.h = coast > -.13 ? -1 : -2;
      } else {
        var deg = ang * 180 / Math.PI + (n2(x / 3 + 20, y / 3 + 20) - .5) * 30;
        var hubR = .27 + (n3(x / 2 + 5, y / 2 + 5) - .5) * .06;
        c.biome = r < hubR ? "hub" : regionAt(deg);
        var st = STYLE[c.biome], f = fbm(x, y);
        var h = c.biome === "hub" ? 3 : st.h(f);
        var cap = 1 + Math.floor(coast * RAD * (st.cap || 1.15));
        c.h = Math.max(1, Math.min(h, cap));
        c.beach = c.h === 1 && st.beach !== false;
        c.path = c.biome === "hub" && (x === 13 || x === 14 || y === 13 || y === 14);
        var arr = st.top;
        c.top = c.beach ? MAT.sand : c.path ? MAT.path : arr[Math.min(arr.length - 1, Math.floor(n3(x * .8 + 3, y * .8 + 9) * arr.length))];
      }
      cols.push(c);
    }
    cols.forEach(function (c) {
      if (c.water) c.foam = [[1, 0], [-1, 0], [0, 1], [0, -1]].some(function (d) { var n = at(c.x + d[0], c.y + d[1]); return n && !n.water; });
    });

    var tallest = {};
    cols.forEach(function (c) {
      var r1 = prand(), r2 = prand();
      if (c.water || c.beach || c.path || c.biome === "hub") return;
      var p = STYLE[c.biome].prop(c.x, c.y, c.h, r1, r2, prand);
      c.props = p;
      if (p) {
        var t = Math.max.apply(null, p.map(function (b) { return b.z + b.h; }));
        if (!tallest[c.biome] || t > tallest[c.biome].top) tallest[c.biome] = { c: c, top: t };
      }
    });
    var portal = at(13, 15);
    if (portal && portal.biome === "hub") {
      var ph = portal.h, stone = "#cabdab";
      portal.props = [
        B(13.25, 12.25, ph, .5, .5, 2.1, stone, null, "lr"),
        B(13.46, 12.75, ph, .08, 2.5, 2.1, "#a192e2", "#b5a8ee", "tr", "#c3b8ff"),
        B(13.25, 15.25, ph, .5, .5, 2.1, stone, null, "lr"),
        B(13.15, 12.15, ph + 2.1, .7, 3.7, .45, stone, "#d9cebc"),
      ];
    }
    [[11, 12], [15, 11], [12, 16], [16, 15]].forEach(function (l) {
      var t = at(l[0], l[1]);
      if (t && t.biome === "hub" && !t.path && !t.props) t.props = P.lantern(l[0], l[1], t.h);
    });
    cols.forEach(function (c) { if (c.props) c.propTop = Math.max.apply(null, c.props.map(function (b) { return b.z + b.h; })) - c.h; });

    function centroid(b) {
      var sx = 0, sy = 0, n = 0;
      cols.forEach(function (c) { if (c.biome === b && !c.beach) { sx += c.x; sy += c.y; n++; } });
      return n ? [sx / n, sy / n] : null;
    }
    function nearest(b, p) {
      var best = null, bd = 1e9;
      cols.forEach(function (c) {
        if (c.biome !== b || c.beach) return;
        var d = Math.hypot(c.x - p[0], c.y - p[1]);
        if (d < bd) { bd = d; best = c; }
      });
      return best;
    }
    var anchors = { hub: portal ? [13.5, 14, portal.h + 2.7] : [CEN, CEN, 4] };
    regions.forEach(function (b) {
      var st = STYLE[b], tl = tallest[b];
      if (st.tall && tl) { anchors[b] = [tl.c.x + .5, tl.c.y + .5, tl.c.h + tl.c.propTop + .3]; return; }
      var ce = centroid(b), c = ce && nearest(b, ce);
      if (c) anchors[b] = [c.x + .5, c.y + .5, c.h + c.propTop + .15];
    });

    var minY = Infinity;
    cols.forEach(function (c) { minY = Math.min(minY, (c.x + c.y) / 2 - (Math.max(c.h, WL) + c.propTop) * 1.2); });
    var stars = [];
    for (var i = 0; i < 150; i++) { var s = rand(); stars.push({ x: rand(), y: rand(), s: s < .18 ? 2 : 1, big: s > .975, a: .35 + rand() * .6 }); }
    return { N: N, WL: WL, FLOOR: FLOOR, cols: cols, at: at, anchors: anchors, stars: stars, bounds: { minY: minY, maxY: N + -FLOOR * 1.2 } };
  }

  /* ── Island rendering ────────────────────────────────────────────────── */
  function createWorld(canvas, stage, tagsEl, horizonEl) {
    var ctx = canvas && canvas.getContext && canvas.getContext("2d");
    if (!ctx) return null;
    var world = null, W = 0, H = 0, u = 10, ox = 0, oy = 0, S = { x: 0, y: 0, w: 1, h: 1 };
    var env = ENV[root.dataset.vxTod] || ENV.day, anim = 0;
    var wideMQ = window.matchMedia("(min-width: 761px)");

    function Pr(x, y, z) { return [ox + (x - y) * u, oy + (x + y) * u / 2 - z * u * 1.2]; }
    function lit(c, f) {
      var L = env.light, a = env.tintAmt, t = env.tint;
      return [(c[0] * L[0] * (1 - a) + t[0] * a) * f, (c[1] * L[1] * (1 - a) + t[1] * a) * f, (c[2] * L[2] * (1 - a) + t[2] * a) * f];
    }
    function glowing(c, f, g) {
      var base = lit(c, f);
      if (env.lamps <= 0) return base;
      var k = .9 + .1 * f;
      return mix(base, [g[0] * k, g[1] * k, g[2] * k], env.lamps);
    }
    function poly(pts, col) {
      ctx.beginPath();
      ctx.moveTo(pts[0][0], pts[0][1]);
      for (var i = 1; i < pts.length; i++) ctx.lineTo(pts[i][0], pts[i][1]);
      ctx.closePath();
      var s = css(col);
      ctx.fillStyle = s; ctx.strokeStyle = s;
      ctx.fill(); ctx.stroke();
    }
    function drawBox(b) {
      var x = b.x, y = b.y, z = b.z, x2 = x + b.w, y2 = y + b.d, z2 = z + b.h;
      function col(c, f) { return b.glow ? glowing(c, f, b.glow) : lit(c, f); }
      if (b.faces.indexOf("l") > -1) poly([Pr(x, y2, z), Pr(x2, y2, z), Pr(x2, y2, z2), Pr(x, y2, z2)], col(b.c, LEFT));
      if (b.faces.indexOf("r") > -1) poly([Pr(x2, y, z), Pr(x2, y2, z), Pr(x2, y2, z2), Pr(x2, y, z2)], col(b.c, RIGHT));
      if (b.faces.indexOf("t") > -1) poly([Pr(x, y, z2), Pr(x2, y, z2), Pr(x2, y2, z2), Pr(x, y2, z2)], col(b.top || b.c, 1));
    }
    function sideCol(c, z) {
      if (z < 0) return MAT.strata[Math.max(0, Math.min(2, z - world.FLOOR))];
      if (c.beach) return MAT.sandSide;
      var st = STYLE[c.biome];
      if (st.layer) return z % 2 ? st.layer[1] : st.layer[0];
      return z === c.h - 1 ? st.edge : st.under;
    }
    function drawColumn(c) {
      var x = c.x, y = c.y, nx = world.at(x + 1, y), ny = world.at(x, y + 1), F = world.FLOOR, WL = world.WL;
      var hx = nx ? nx.h : F, hy = ny ? ny.h : F;
      for (var z = F; z < c.h; z++) {
        var above = z + 1 > WL, ao = .8 + .2 * Math.min(1, (z - F) / 8);
        if (z >= hy && (!ny || above)) poly([Pr(x, y + 1, z), Pr(x + 1, y + 1, z), Pr(x + 1, y + 1, z + 1), Pr(x, y + 1, z + 1)], lit(sideCol(c, z), LEFT * ao));
        if (z >= hx && (!nx || above)) poly([Pr(x + 1, y, z), Pr(x + 1, y + 1, z), Pr(x + 1, y + 1, z + 1), Pr(x + 1, y, z + 1)], lit(sideCol(c, z), RIGHT * ao));
      }
      if (c.water) {
        if (!ny) poly([Pr(x, y + 1, c.h), Pr(x + 1, y + 1, c.h), Pr(x + 1, y + 1, WL), Pr(x, y + 1, WL)], lit(MAT.waterSide, LEFT));
        if (!nx) poly([Pr(x + 1, y, c.h), Pr(x + 1, y + 1, c.h), Pr(x + 1, y + 1, WL), Pr(x + 1, y, WL)], lit(MAT.waterSide, RIGHT));
        var wc = c.foam ? MAT.foam : c.h === -1 ? MAT.shallow : MAT.deep;
        poly([Pr(x, y, WL), Pr(x + 1, y, WL), Pr(x + 1, y + 1, WL), Pr(x, y + 1, WL)], lit(wc, c.jit));
      } else {
        poly([Pr(x, y, c.h), Pr(x + 1, y, c.h), Pr(x + 1, y + 1, c.h), Pr(x, y + 1, c.h)], lit(c.top, c.jit));
      }
    }
    function drawIsland() {
      var N = world.N;
      ctx.lineWidth = .7; ctx.lineJoin = "round";
      for (var s = 0; s <= 2 * (N - 1); s++) {
        for (var x = Math.max(0, s - N + 1); x <= Math.min(s, N - 1); x++) {
          var c = world.cols[x * N + (s - x)];
          drawColumn(c);
          if (!c.props) continue;
          c.props.forEach(drawBox);
          if (c.props.win) c.props.win.forEach(function (w) {
            var glass = lit(MAT.glass, w.f);
            poly(w.q.map(function (p) { return Pr(p[0], p[1], p[2]); }), w.lit && env.lamps > 0 ? mix(glass, w.lamp, env.lamps) : glass);
          });
        }
      }
    }
    function cubeAt(cx, cy, s, cc, a) {
      ctx.globalAlpha = a;
      var hh = s / 2, ch = s * 1.2;
      poly([[cx, cy - hh], [cx + s, cy], [cx, cy + hh], [cx - s, cy]], cc[0]);
      poly([[cx - s, cy], [cx, cy + hh], [cx, cy + hh + ch], [cx - s, cy + ch]], cc[1]);
      poly([[cx, cy + hh], [cx + s, cy], [cx + s, cy + ch], [cx, cy + hh + ch]], cc[2]);
      ctx.globalAlpha = 1;
    }
    var CLOUDS = [
      { fx: .02, fy: .1, k: 1, shape: ["   ####   ", " ######## ", "##########"] },
      { fx: .62, fy: .02, k: .8, shape: ["  ###  ", "#######"] },
      { fx: .34, fy: .2, k: .65, shape: [" ### ", "#####"] },
    ];
    function drawSky() {
      var g = ctx.createLinearGradient(0, 0, 0, H), stops = [0, .5, .8, 1];
      env.sky.forEach(function (c, i) { g.addColorStop(stops[i], css(c)); });
      ctx.fillStyle = g; ctx.fillRect(0, 0, W, H);
      if (env.stars > .01) {
        world.stars.forEach(function (s) {
          ctx.fillStyle = "rgba(255,250,238," + (s.a * env.stars).toFixed(3) + ")";
          var px = Math.round(s.x * W), py = Math.round(s.y * H * .78);
          if (s.big) { ctx.fillRect(px - 2, py, 5, 1.5); ctx.fillRect(px, py - 2, 1.5, 5); }
          else ctx.fillRect(px, py, s.s, s.s);
        });
      }
      var sz = Math.max(9, Math.min(24, S.w * .028));
      if (env.sunA > .01) cubeAt(S.x + S.w * env.sunX, S.y + S.h * env.sunY, sz, env.sunCol, env.sunA);
      if (env.moonA > .01) {
        var mx = S.x + S.w * .84, my = S.y + S.h * .06, d = sz * .22;
        cubeAt(mx, my, sz * .9, MAT.moon, env.moonA);
        ctx.globalAlpha = env.moonA;
        poly([[mx - sz * .3, my - d / 2], [mx - sz * .3 + d, my], [mx - sz * .3, my + d / 2], [mx - sz * .3 - d, my]], MAT.crater);
        poly([[mx + sz * .28, my + sz * .05 - d / 3], [mx + sz * .28 + d * .7, my + sz * .05], [mx + sz * .28, my + sz * .05 + d / 3], [mx + sz * .28 - d * .7, my + sz * .05]], MAT.crater);
        ctx.globalAlpha = 1;
      }
      var bu = Math.max(4, Math.round(S.w * .011));
      ctx.globalAlpha = env.cloudA;
      CLOUDS.forEach(function (cl) {
        var x0 = Math.round(S.x + S.w * cl.fx), y0 = Math.round(S.y + S.h * cl.fy), b = Math.max(3, Math.round(bu * cl.k)), last = cl.shape.length - 1;
        [false, true].forEach(function (shade) {
          ctx.beginPath();
          cl.shape.forEach(function (row, r) {
            if ((r === last) !== shade) return;
            for (var i = 0; i < row.length; i++) if (row[i] === "#") ctx.rect(x0 + i * b, y0 + r * b, b, b);
          });
          ctx.fillStyle = css(shade ? env.cloudShade : env.cloud);
          ctx.fill();
        });
      });
      ctx.globalAlpha = 1;
    }
    function render() {
      if (!W || !H || !world) return;
      ctx.clearRect(0, 0, W, H);
      drawSky();
      drawIsland();
    }

    function placeTags() {
      if (!world) return;
      var items = $$(".vx-tag", tagsEl);
      if (!wideMQ.matches) return;
      var ur = tagsEl.getBoundingClientRect(), cr = canvas.getBoundingClientRect();
      var offX = cr.left - ur.left, offY = cr.top - ur.top, UW = ur.width;
      var list = items.map(function (li) {
        var a = world.anchors[li.dataset.anchor] || world.anchors.hub, p = Pr(a[0], a[1], a[2]), card = $(".vx-tag-card", li);
        var w = card.offsetWidth, h = card.offsetHeight, al = li.dataset.align;
        return { li: li, x: p[0] + offX, y: p[1] + offY, w: w, h: h, stem: 16, dx: al === "start" ? w / 2 - 14 : al === "end" ? -(w / 2 - 14) : 0 };
      });
      function box(t) { return { l: t.x + t.dx - t.w / 2, r: t.x + t.dx + t.w / 2, b: t.y - t.stem - 5, t: t.y - t.stem - 5 - t.h }; }
      list.forEach(function (t) {
        var bx = box(t);
        if (bx.l < 8) t.dx += 8 - bx.l;
        if (bx.r > UW - 8) t.dx -= bx.r - (UW - 8);
      });
      for (var pass = 0; pass < 10; pass++) {
        var moved = false;
        for (var i = 0; i < list.length; i++) for (var j = i + 1; j < list.length; j++) {
          var A = box(list[i]), Bx = box(list[j]);
          if (A.l < Bx.r + 8 && Bx.l < A.r + 8 && A.t < Bx.b + 6 && Bx.t < A.b + 6) {
            var hi = list[i].y <= list[j].y ? list[i] : list[j], lo = hi === list[i] ? list[j] : list[i];
            hi.stem = Math.min(190, hi.stem + (box(hi).b - box(lo).t) + 6);
            moved = true;
          }
        }
        if (!moved) break;
      }
      list.forEach(function (t) {
        t.li.style.setProperty("--x", t.x.toFixed(1) + "px");
        t.li.style.setProperty("--y", t.y.toFixed(1) + "px");
        t.li.style.setProperty("--stem", Math.max(10, t.stem).toFixed(0) + "px");
        t.li.style.setProperty("--dx", t.dx.toFixed(1) + "px");
      });
    }
    function drawHorizon() {
      if (!horizonEl) return;
      var w = Math.max(1, Math.ceil(W)), hh = 40, step = w < 640 ? 12 : 18, d = "M0 " + hh;
      for (var i = 0, x = 0; x < w; i++, x += step) {
        var v = Math.sin(i * .21) * .5 + Math.sin(i * .077 + 1.3) * .5;
        var y = hh - 5 - Math.round((v * .5 + .5) * 3) * 6;
        d += "L" + x + " " + y + "L" + (x + step) + " " + y;
      }
      d += "L" + w + " " + hh + "Z";
      horizonEl.setAttribute("viewBox", "0 0 " + w + " " + hh);
      $("path", horizonEl).setAttribute("d", d);
    }
    function layout() {
      var cr = canvas.getBoundingClientRect();
      W = cr.width; H = cr.height;
      if (!W || !H || !world) return;
      var dpr = Math.min(window.devicePixelRatio || 1, 2.5);
      canvas.width = Math.round(W * dpr); canvas.height = Math.round(H * dpr);
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      var sr = stage.getBoundingClientRect();
      S = { x: sr.left - cr.left, y: sr.top - cr.top, w: sr.width, h: sr.height };
      var topPad = wideMQ.matches ? 64 : 4, availH = Math.max(40, S.h - topPad), spanY = world.bounds.maxY - world.bounds.minY;
      u = Math.min(S.w / (2 * world.N), availH / spanY);
      ox = S.x + S.w / 2;
      oy = S.y + topPad + (availH - spanY * u) / 2 - world.bounds.minY * u;
      render();
      placeTags();
      drawHorizon();
    }
    function setRegions(regions) {
      var key = regions.join(",");
      if (world && world.key === key) return;
      world = genWorld(regions, hashStr("btt|" + key));
      world.key = key;
      layout();
    }
    function setEnv(name, animate) {
      var target = ENV[name];
      if (!target) return;
      cancelAnimationFrame(anim);
      if (!animate) { env = target; render(); return; }
      var from = env, t0 = performance.now();
      function step(t) {
        var k = Math.min(1, (t - t0) / 600), e = 1 - Math.pow(1 - k, 3);
        env = lerpEnv(from, target, e);
        render();
        if (k < 1) anim = requestAnimationFrame(step);
      }
      anim = requestAnimationFrame(step);
    }
    return { layout: layout, setEnv: setEnv, placeTags: placeTags, setRegions: setRegions };
  }

  /* ── Iso boxes as inline SVG: podiums + the giveaway gift ───────────── */
  function isoSVG(boxes, s, extra) {
    function Pp(x, y, z) { return [(x - y) * s, (x + y) * s / 2 - z * s * 1.2]; }
    var x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity, out = [];
    function grow(p) { x0 = Math.min(x0, p[0]); y0 = Math.min(y0, p[1]); x1 = Math.max(x1, p[0]); y1 = Math.max(y1, p[1]); }
    function add(pts, fill) {
      pts.forEach(grow);
      out.push('<path d="M' + pts.map(function (p) { return p[0].toFixed(1) + " " + p[1].toFixed(1); }).join("L") + 'Z" style="fill:' + fill + ";stroke:" + fill + '"/>');
    }
    boxes.forEach(function (b) {
      var f = b.faces || "tlr", x = b.x, y = b.y, z = b.z, x2 = x + b.w, y2 = y + b.d, z2 = z + b.h;
      var L = "color-mix(in srgb," + b.c + " 80%,#16121e)", R = "color-mix(in srgb," + b.c + " 63%,#16121e)";
      if (f.indexOf("l") > -1) add([Pp(x, y2, z), Pp(x2, y2, z), Pp(x2, y2, z2), Pp(x, y2, z2)], L);
      if (f.indexOf("r") > -1) add([Pp(x2, y, z), Pp(x2, y2, z), Pp(x2, y2, z2), Pp(x2, y, z2)], R);
      if (f.indexOf("t") > -1) add([Pp(x, y, z2), Pp(x2, y, z2), Pp(x2, y2, z2), Pp(x, y2, z2)], b.top || b.c);
    });
    (extra || []).forEach(function (p) { grow(Pp(p[0], p[1], p[2])); });
    return { body: '<g stroke-width=".6" stroke-linejoin="round">' + out.join("") + "</g>", vb: [x0 - 1, y0 - 1, x1 - x0 + 2, y1 - y0 + 2].map(function (v) { return v.toFixed(1); }).join(" ") };
  }

  /* ── Rotations: the island, Today and Who's in town ─────────────────── */
  var MERCHANT_COLOR = { corruxion: "#9b6fd0", fluxion: "#5fb6c9", luxion: "#e8b64c", wild_mana: "#6fb4d8", d15: "#7c86b8", stampy: "#ec94b5" };
  function mColor(m) { return MERCHANT_COLOR[m.id] || "#8f99b5"; }
  var ISLAND_ORDER = ["d15", "wild_mana", "stampy"];
  var merchants = [], targets = {}, world = null;

  function plan(list) {
    var regions = [], byId = {};
    list.forEach(function (m) { byId[m.id] = m; });
    ISLAND_ORDER.forEach(function (id) {
      var m = byId[id];
      if (!m || !m.active) return;
      (m.biomes || []).forEach(function (b) {
        if (b && STYLE[b.icon] && b.icon !== "hub" && regions.indexOf(b.icon) < 0 && regions.length < 6) regions.push(b.icon);
      });
    });
    regions.sort(function (a, b) { return (STYLE[b].tall ? 1 : 0) - (STYLE[a].tall ? 1 : 0); });
    if (!regions.length) regions = ["tundra", "neon", "candy", "fae", "undead"];
    // One label per biome on the island, named in full, with a row for every
    // rotation happening there; merchants with no biome share one card at the Hub.
    var tags = [];
    regions.forEach(function (icon) {
      var rows = [], name = null;
      ISLAND_ORDER.forEach(function (id) {
        var m = byId[id];
        if (!m || !m.active) return;
        (m.biomes || []).forEach(function (b) {
          if (!b || b.icon !== icon) return;
          if (!name) name = b.name;
          if (rows.indexOf(m) < 0) rows.push(m);
        });
      });
      if (name) tags.push({ anchor: icon, title: name, color: STYLE[icon].marker, rows: rows });
    });
    var hub = list.filter(function (m) { return m.active && !(m.biomes && m.biomes.length); });
    if (hub.length) tags.push({ anchor: "hub", title: null, color: STYLE.hub.marker, rows: hub });
    return { regions: regions, tags: tags };
  }

  // What each merchant's countdown points at, and how it's labelled.
  function change(m) {
    var w = m.window;
    if (m.active && w && w.open && w.ends_at) return { at: w.ends_at, label: tr("closes"), line: tr("open · closes in") };
    if (m.active && w && !w.open && w.starts_at) return { at: w.starts_at, label: tr("opens"), line: tr("next window in") };
    var rotating = m.biomes && m.biomes.length && m.id !== "stampy";
    if (m.active && m.ends_at) return { at: m.ends_at, label: rotating ? tr("rotates") : tr("leaves"), line: rotating ? tr("changes in") : tr("leaves in") };
    if (!m.active && m.starts_at) return { at: m.starts_at, label: tr("back"), line: tr("back in") };
    return null;
  }
  function iconImgs(bs) {
    return (bs || []).filter(function (b) { return b.icon && b.icon !== "unknown"; }).slice(0, 3).map(function (b) {
      return '<img src="/static/assets/biomes/' + encodeURIComponent(b.icon) + '.png" alt="" width="28" height="28" loading="lazy">';
    }).join("");
  }
  function cube(color, cls) {
    return '<svg class="vx-cube' + (cls ? " " + cls : "") + '" aria-hidden="true" style="--c:' + color + '"><use href="#vx-cube"/></svg>';
  }

  // Every label says what is there in words, no key needed: a biome's full name over
  // the rotations running in it, or a Hub merchant over when it leaves. The list under
  // the search keeps the countdowns.
  function renderTags(p) {
    var el = $("#vx-tags");
    if (!el) return;
    var cards = [];
    p.tags.forEach(function (t) {
      if (t.title) {
        cards.push({ anchor: t.anchor, color: t.color, title: esc(t.title),
          sub: esc(t.rows.map(function (m) { return m.name; }).join(" · ")) });
      } else {
        t.rows.forEach(function (m) {
          var ch = change(m);
          cards.push({ anchor: t.anchor, color: mColor(m), title: esc(m.name),
            sub: esc(ch ? ch.line : tr("here")) + (ch ? ' <span class="vx-cd" data-vx-cd="m:' + esc(m.id) + '"></span>' : "") });
        });
      }
    });
    el.innerHTML = cards.map(function (c, i) {
      var align = i % 3 === 1 ? "start" : i % 3 === 2 ? "end" : "center";
      return '<li class="vx-tag" data-anchor="' + esc(c.anchor) + '" data-align="' + align + '" style="--c:' + c.color + '">' +
        '<div class="vx-tag-card"><strong>' + c.title + '</strong><span class="vx-tag-sub">' + c.sub + "</span></div>" +
        '<span class="vx-tag-stem" aria-hidden="true"></span><svg class="vx-tag-pin" aria-hidden="true"><use href="#vx-pin"/></svg></li>';
    }).join("");
    var legend = $("#vx-legend");
    if (!legend) return;
    var shown = [];
    p.tags.forEach(function (t) { t.rows.forEach(function (m) { if (shown.indexOf(m) < 0) shown.push(m); }); });
    legend.hidden = !shown.length;
    legend.innerHTML = shown.map(function (m) {
      var ch = change(m), cd = ch ? '<span class="vx-cd" data-vx-cd="m:' + esc(m.id) + '"></span>' : "";
      var when = esc(ch ? ch.line : tr("here")) + " " + cd;
      return "<li>" + cube(mColor(m)) + "<b>" + esc(m.name) + '</b><span class="vx-legend-when">' + when + "</span></li>";
    }).join("");
  }

  function nextBiomes(m) {
    var ch = change(m), from = ch ? ch.at : 0;
    if (!m.active && m.biomes && m.biomes.length) return m.biomes;
    var s = (m.schedule || []).filter(function (x) { return x.starts_at >= from - 60 && x.biomes && x.biomes.length; })[0];
    return s ? s.biomes : null;
  }
  function renderTable() {
    var body = $("#vx-merchants");
    if (!body || !merchants.length) return;
    body.innerHTML = merchants.map(function (m) {
      var ch = change(m), where;
      if (m.active && m.biomes && m.biomes.length) {
        where = '<span class="vx-icons">' + iconImgs(m.biomes) + "</span>" + esc(m.biomes.map(function (b) { return b.name; }).join(", "));
      } else {
        where = '<span class="vx-away">' + esc(m.active ? (m.state || tr("In town")) : tr("Away")) + "</span>";
      }
      var nb = nextBiomes(m);
      return "<tr>" +
        '<td class="vx-p"><div class="vx-who">' + cube(mColor(m)) + "<b>" + esc(m.name) + "</b></div></td>" +
        '<td class="vx-w"><div class="vx-where">' + where + "</div></td>" +
        '<td class="vx-l">' + (ch ? '<span class="vx-when"><span data-vx-cd="m:' + esc(m.id) + '"></span><small>' + esc(ch.label) + "</small></span>" : "") + "</td>" +
        '<td class="vx-n">' + (nb ? esc(nb.map(function (b) { return b.name; }).join(", ")) : "") + "</td>" +
        '<td class="vx-tl-col"><div class="vx-tl" data-vx-tl="' + esc(m.id) + '"></div></td></tr>';
    }).join("");
    renderTimeline();
    tickCountdowns();
  }
  function renderTimeline() {
    var n = now() / 1000, span = 12 * 3600;
    merchants.forEach(function (m) {
      var el = $('.vx-tl[data-vx-tl="' + m.id + '"]');
      if (!el) return;
      var wins = [];
      if (m.active && m.ends_at) wins.push([n, m.ends_at]);
      else if (!m.active && m.starts_at) wins.push([m.starts_at, m.ends_at || m.starts_at + 3600]);
      (m.schedule || []).forEach(function (s) {
        if (s.starts_at && s.ends_at && s.ends_at > n && !wins.some(function (w) { return Math.abs(w[0] - s.starts_at) < 60 || (s.starts_at < w[1] && s.ends_at <= w[1]); })) wins.push([s.starts_at, s.ends_at]);
      });
      el.innerHTML = wins.map(function (w, i) {
        var a = Math.max(0, w[0] - n), b = Math.min(span, w[1] - n);
        if (b - a <= 0) return "";
        var open = w[1] - n > span;
        return '<i class="' + (open ? "open" : "") + '" style="left:' + (a / span * 100).toFixed(2) + "%;width:calc(" + ((b - a) / span * 100).toFixed(2) + "% - " + (i ? 2 : 0) + "px);--c:" + mColor(m) + (i ? ";opacity:.62" : "") + '"></i>';
      }).join("");
    });
    var ticks = $("#vx-ticks");
    if (ticks) {
      var out = ['<span>' + esc(tr("Now")) + "</span>"];
      for (var i = 1; i <= 4; i++) {
        var t = server(now() + i * 3 * HOUR);
        out.push('<span style="left:' + i * 25 + '%">' + pad(t.getUTCHours()) + ":" + pad(t.getUTCMinutes()) + "</span>");
      }
      ticks.innerHTML = out.join("");
    }
  }

  var rotations = null;
  function renderToday() {
    var d = rotations;
    if (!d) return;
    var db = d.daily_buff;
    if (db && db.name) {
      $("#vx-today-day").textContent = db.name;
      var fd = $("#vx-facts-day");
      if (fd) fd.textContent = db.name;
      function items(bs) { return (bs || []).map(function (b) { return "<li>" + cube("var(--vx-accent)") + "<span>" + esc(b) + "</span></li>"; }).join(""); }
      $("#vx-today-bonuses").innerHTML = items(db.normal_buffs);
      $("#vx-today-patron").innerHTML = items(db.premium_buffs);
      $("#vx-tier").hidden = !(db.premium_buffs || []).length;
    }
    var week = $("#vx-week");
    if (week && d.daily_rotation && d.daily_rotation.length) {
      week.innerHTML = d.daily_rotation.map(function (x) {
        var wd = typeof x.weekday === "string" ? x.weekday.slice(0, 3) : "";
        return "<li" + (x.is_current ? ' aria-current="date"' : "") + "><b>" + esc(wd) + "</b>" + esc(String(x.name || "").replace(/ Day$/, "")) + "</li>";
      }).join("");
    }
    var wb = d.weekly_buff, wEl = $("#vx-weekly");
    if (wb && wb.name && wEl) {
      wEl.querySelector("strong").textContent = wb.name;
      var wt = $("#vx-weekly-text");
      if (!wt) { wt = document.createElement("span"); wt.id = "vx-weekly-text"; wEl.insertBefore(wt, wEl.querySelector("strong").nextSibling); wEl.insertBefore(document.createTextNode(" "), wt.nextSibling); }
      wt.textContent = (wb.buffs || []).join(" · ");
    }
    var ch = d.chaos, cEl = $("#vx-chaos");
    if (ch && ch.item && ch.item.name && cEl) {
      cEl.querySelector("strong").textContent = ch.item.name;
      var cd = cEl.querySelector("[data-vx-cd]");
      if (cd && ch.ends_at) cd.dataset.vxAt = ch.ends_at;
    }
  }

  function applyRotations(d) {
    rotations = d;
    var st = d.server_time || {};
    if (typeof st.now_unix === "number") {
      var off = st.now_unix * 1000 - Date.now();
      clockOffset = Math.abs(off) < 10 * 60e3 ? off : 0;
    }
    targets.daily = st.daily_reset_at ? st.daily_reset_at * 1000 : null;
    targets.weekly = st.weekly_reset_at ? st.weekly_reset_at * 1000 : null;
    merchants = (d.merchants || []).map(function (m) {
      return { id: m.id, name: m.name, active: !!m.active, state: m.state, starts_at: m.starts_at, ends_at: m.ends_at,
        biomes: m.biomes || [], window: m.window || null, schedule: m.schedule || [] };
    });
    renderAll();
  }
  function renderAll() {
    var p = plan(merchants);
    renderTags(p);
    if (world) world.setRegions(p.regions);
    renderTable();
    renderToday();
    tickCountdowns();
    if (world) world.placeTags();
  }

  /* ── Records ─────────────────────────────────────────────────────────── */
  var RECORD = {
    trove_mastery: { per: 100, c: "#e2ae4c", note: function () { return tr("One block is 100 levels"); } },
    geode_mastery: { per: 10, c: "#63b0b4", note: function () { return tr("One block is 10 levels"); } },
    power_rank: { per: 5000, c: "#de7a60", note: function () { return tr("One block is 5,000 power rank"); } },
  };
  function drawPodium(li) {
    var spec = RECORD[li.dataset.key], raw = parseFloat(li.dataset.raw);
    var pod = $(".vx-podium", li);
    if (!spec || !pod || !(raw > 0)) return;
    var n = Math.min(12, raw / spec.per), c = spec.c, light = "color-mix(in srgb," + c + " 84%,#fff)";
    var boxes = [{ x: 0, y: 0, z: 0, w: 1.8, d: 1.8, h: .45, c: "var(--vx-plinth)" }], full = Math.floor(n + 1e-6);
    for (var i = 0; i < full; i++) boxes.push({ x: .4, y: .4, z: .45 + i, w: 1, d: 1, h: 1, c: i % 2 ? c : light });
    if (n - full > .02) boxes.push({ x: .4, y: .4, z: .45 + full, w: 1, d: 1, h: n - full, c: full % 2 ? c : light });
    var r = isoSVG(boxes, 20, [[.4, .4, 12.6]]);
    pod.innerHTML = '<svg viewBox="' + r.vb + '" focusable="false">' + r.body + "</svg>";
    var body = pod.nextElementSibling, sc = body && $(".vx-scale", body);
    if (body && !sc) { sc = document.createElement("p"); sc.className = "vx-scale"; body.appendChild(sc); }
    if (sc) sc.innerHTML = cube(c) + esc(spec.note());
  }
  function renderRecords(d) {
    var box = $("#vx-records");
    if (!box || !d) return;
    var rows = [];
    [["trove_mastery", "Trove Mastery", true], ["geode_mastery", "Geode Mastery", true], ["power_rank", "Power Rank", false]].forEach(function (k) {
      var r = d[k[0]];
      if (!r) return;
      var raw = k[2] ? r.level : r.value;
      var value = k[2] ? tr("Level") + " " + num(r.level) + (r.points != null ? " · " + num(r.points) + " " + tr("points") : "") : num(r.value);
      var holder = r.player_name ? '<a href="/player/' + encodeURIComponent(r.player_name) + '">' + esc(r.player_name) + "</a>" : "—";
      rows.push('<li class="vx-record" data-key="' + k[0] + '" data-raw="' + (raw == null ? "" : raw) + '"><div class="vx-podium" aria-hidden="true"></div><div>' +
        '<p class="vx-metric">' + esc(tr(k[1])) + '</p><p class="vx-holder">' + holder + '</p><p class="vx-value">' + esc(value) + "</p></div></li>");
    });
    if (rows.length) box.innerHTML = rows.join("");
    $$(".vx-record", box).forEach(drawPodium);
  }

  /* ── Servers, players, latest update, giveaway ──────────────────────── */
  var statusData = null, activityData = null, giveaway = null;
  function renderStatus() {
    var dd = $("#vx-servers"), txt = $("#vx-status-text");
    if (dd && txt && statusData) {
      var up = statusData.overall === "online", down = statusData.overall === "down";
      dd.classList.toggle("vx-servers-up", up);
      dd.classList.toggle("vx-servers-down", down);
      txt.textContent = up ? tr("Online") : down ? tr("Down") : tr("Unknown");
    }
    var pl = $("#vx-players");
    if (pl && activityData) {
      var a = activityData;
      pl.innerHTML = "<b>" + num(a.estimate) + "</b> " + esc(tr("players active in the last hour,")) + " <b>" + num(a.estimate_24h) + "</b> " +
        esc(tr("in the last 24 hours and")) + " <b>" + num(a.estimate_7d) + "</b> " + esc(tr("in the last 7 days."));
    }
  }
  function renderGiveaway() {
    var sec = $("#giveaways");
    if (!sec) return;
    if (!giveaway) { sec.hidden = true; return; }
    $("#vx-give-title").textContent = tr("Win") + " " + (giveaway.prize_name || giveaway.prize || giveaway.title || tr("a prize"));
    var entries = typeof giveaway.entry_count === "number" ? giveaway.entry_count : giveaway.entries;
    var ends = giveaway.ends_at ? (typeof giveaway.ends_at === "number" ? giveaway.ends_at * 1000 : Date.parse(giveaway.ends_at)) : null;
    var parts = [];
    if (typeof entries === "number") parts.push("<b>" + num(entries) + "</b> " + esc(tr("entries so far.")));
    if (ends && !isNaN(ends)) { targets.giveaway = ends; parts.push(esc(tr("Entries close in")) + ' <b data-vx-cd="giveaway"></b>.'); }
    $("#vx-give-meta").innerHTML = parts.join(" ");
    var gift = $("#vx-gift");
    if (gift && !gift.childNodes.length) {
      var r = isoSVG([
        { x: -.3, y: -.3, z: -.3, w: 2.6, d: 2.6, h: .3, c: "var(--vx-plinth)" },
        { x: 0, y: 0, z: 0, w: 2, d: 2, h: 1.3, c: "#e07c93" },
        { x: .82, y: -.01, z: 0, w: .36, d: 2.02, h: 1.31, c: "#f2c46b", faces: "tl" },
        { x: -.01, y: .82, z: 0, w: 2.02, d: .36, h: 1.31, c: "#f2c46b", faces: "tr" },
        { x: .52, y: .86, z: 1.31, w: .3, d: .3, h: .3, c: "#f2c46b" },
        { x: .88, y: .88, z: 1.31, w: .24, d: .24, h: .2, c: "#e6ad4f" },
        { x: 1.18, y: .86, z: 1.31, w: .3, d: .3, h: .3, c: "#f2c46b" },
      ], 34);
      gift.setAttribute("viewBox", r.vb);
      gift.innerHTML = r.body;
    }
    sec.hidden = false;
    tickCountdowns();
  }

  /* ── News, videos, mods: server-rendered; fetched only if that came up empty ── */
  function fillList(sel, url, pick, row, empty) {
    var el = $(sel);
    if (!el || el.children.length) return;
    getJSON(url).then(function (d) {
      var items = pick((d && d.items) || []);
      el.innerHTML = items.length ? items.map(row).join("") : '<li class="vx-empty">' + esc(tr(empty)) + "</li>";
    }).catch(function () {});
  }
  function ago(v) { return U.timeAgo ? U.timeAgo(v) || "" : ""; }
  var MOD_COLORS = ["#7a82b8", "#e79a7c", "#8fc3a3", "#e6bf62", "#c58fd0", "#6fb4d8"];
  function fillLists() {
    fillList("#vx-news", "/site/feeds/news", function (xs) {
      return xs.filter(function (n) { return (n.categories || []).indexOf("Shop Offers") < 0; }).slice(0, 5);
    }, function (n) {
      return '<li><a href="' + esc(n.url || "#") + '" target="_blank" rel="noopener"><span class="vx-t">' + esc(n.title) + '</span><span class="vx-meta">' +
        [n.category ? cube("var(--vx-accent)") + esc(n.category) : "", esc(ago(n.published_at))].filter(Boolean).join(" · ") + "</span></a></li>";
    }, "No news right now.");
    fillList("#vx-videos", "/site/feeds/videos?platform=youtube", function (xs) { return xs.slice(0, 4); }, function (v) {
      var th = (v.thumbnail_url || v.thumbnail || "").replace("{width}", "440").replace("{height}", "248");
      return '<li><a href="' + esc(v.url || "#") + '" target="_blank" rel="noopener"><span class="vx-thumb">' +
        (th ? '<img src="' + esc(th) + '" alt="" loading="lazy" decoding="async" width="148" height="83">' : "") +
        '</span><span><span class="vx-t">' + esc(v.title) + '</span><span class="vx-meta">' + esc(v.channel || "") + "</span></span></a></li>";
    }, "No videos right now.");
    fillList("#vx-mods", "/site/mods/projects?sort=recent&limit=40", function (xs) {
      var per = {}, out = [];
      xs.forEach(function (m) {
        if (out.length >= 6 || !m.handle || !m.slug) return;
        var k = String(m.author || m.owner_username || m.handle).toLowerCase();
        if ((per[k] || 0) >= 2) return;
        per[k] = (per[k] || 0) + 1;
        out.push(m);
      });
      return out;
    }, function (m, i) {
      var by = m.author || m.owner_username || "";
      return '<li><a href="/mods/' + encodeURIComponent(m.handle) + "/" + encodeURIComponent(m.slug) + '">' + cube(MOD_COLORS[i % 6]) +
        "<span><b>" + esc(m.title || m.slug) + "</b><span>" + (by ? esc(tr("by")) + " " + esc(by) : "") + "</span></span></a></li>";
    }, "No mods yet.");
  }

  /* ── Countdowns + time of day ────────────────────────────────────────── */
  function untilDaily(t) {
    if (targets.daily && targets.daily > t) return targets.daily - t;
    var s = server(t);
    return DAY - (((s.getUTCHours() * 60 + s.getUTCMinutes()) * 60 + s.getUTCSeconds()) * 1000 + s.getUTCMilliseconds());
  }
  function untilWeekly(t) {
    if (targets.weekly && targets.weekly > t) return targets.weekly - t;
    var s = server(t), d = (1 - s.getUTCDay() + 7) % 7 || 7;
    return d * DAY - (((s.getUTCHours() * 60 + s.getUTCMinutes()) * 60 + s.getUTCSeconds()) * 1000);
  }
  function tickCountdowns() {
    var t = now();
    $$("[data-vx-cd]").forEach(function (el) {
      var k = el.dataset.vxCd, ms = null;
      if (k === "daily") ms = untilDaily(t);
      else if (k === "weekly") ms = untilWeekly(t);
      else if (k === "chaos") ms = el.dataset.vxAt ? el.dataset.vxAt * 1000 - t : null;
      else if (k === "giveaway") ms = targets.giveaway ? targets.giveaway - t : null;
      else if (k.indexOf("m:") === 0) {
        var m = merchants.filter(function (x) { return x.id === k.slice(2); })[0], ch = m && change(m);
        ms = ch ? ch.at * 1000 - t : null;
      }
      el.textContent = fmt(ms);
    });
  }
  function wantedTod(t) {
    var th = window.BTTAppearance ? window.BTTAppearance.theme() : root.dataset.vxTheme;
    return th === "light" ? "day" : th === "dark" ? "night" : todFor(server(t).getUTCHours());
  }
  function applyTod(name, animate) {
    if (root.dataset.vxTod === name) return;
    root.dataset.vxTod = name;
    if (world) world.setEnv(name, animate && !reduce.matches);
  }

  var lastSlow = 0, lastMinute = -1;
  function tick() {
    var t = now(), s = server(t);
    var hms = pad(s.getUTCHours()) + ":" + pad(s.getUTCMinutes()) + ":" + pad(s.getUTCSeconds());
    var clock = $("#vx-clock");
    if (clock) clock.textContent = hms;
    tickCountdowns();
    applyTod(wantedTod(t), true);
    if (s.getUTCMinutes() !== lastMinute) { lastMinute = s.getUTCMinutes(); renderTimeline(); }
    if (t - lastSlow > 5 * 60e3) {
      lastSlow = t;
      if (lastSlow && rotations) getJSON("/site/rotations").then(applyRotations).catch(function () {});
    }
  }

  /* ── Boot ────────────────────────────────────────────────────────────── */
  function boot() {
    world = createWorld($("#vx-world"), $("#vx-stage"), $("#vx-tags"), $("#vx-horizon"));
    var seed = {};
    try { seed = JSON.parse(($("#vx-island-data") || {}).textContent || "{}") || {}; } catch (_) { seed = {}; }
    if (seed.merchants) {
      targets.daily = seed.daily_reset_at ? seed.daily_reset_at * 1000 : null;
      targets.weekly = seed.weekly_reset_at ? seed.weekly_reset_at * 1000 : null;
      merchants = seed.merchants;
    }
    renderAll();
    if (world && !seed.merchants) world.setRegions(plan([]).regions);
    $$(".vx-record").forEach(drawPodium);

    if (world) {
      var raf = 0;
      function schedule() { cancelAnimationFrame(raf); raf = requestAnimationFrame(function () { world.layout(); }); }
      if ("ResizeObserver" in window) new ResizeObserver(schedule).observe($(".vx-hero"));
      else window.addEventListener("resize", schedule);
      if (document.fonts && document.fonts.ready) document.fonts.ready.then(schedule);
    }
    lastSlow = now();
    tick();
    setInterval(tick, 1000);

    getJSON("/site/rotations").then(applyRotations).catch(function () {});
    fillLists();
    if ($("#vx-records")) getJSON("/site/leaderboards/records").then(function (d) { recordsData = d; renderRecords(d); }).catch(function () {});
    if ($("#vx-servers")) getJSON("/site/trove-status").then(function (d) { statusData = d; renderStatus(); }).catch(function () {});
    if ($("#vx-players")) getJSON("/site/leaderboards/activity").then(function (d) { activityData = d; renderStatus(); }).catch(function () {});
    if ($("#vx-patch")) getJSON("/site/updates/live-us/versions?limit=1").then(function (d) {
      var it = d && d.items && d.items[0], tag = it && (it.version_tag || it.ordinal);
      if (!tag) return;
      $("#vx-patch-text").textContent = tag;
      $("#vx-patch").hidden = false;
    }).catch(function () {});
    if ($("#giveaways")) getJSON("/site/giveaways").then(function (d) {
      giveaway = ((d && d.items) || []).filter(function (g) { return !g.status || ["open", "ongoing", "active"].indexOf(g.status) > -1; })[0] || null;
      renderGiveaway();
    }).catch(function () {});

    var nav = $(".navbar");
    function onScroll() { if (nav) nav.classList.toggle("scrolled", window.scrollY > 12); }
    window.addEventListener("scroll", onScroll, { passive: true });
    onScroll();

    try {
      if (localStorage.getItem("btt-bonus-tier") === "patron") $("#vx-tier-patron").checked = true;
    } catch (_) {}
    $$('input[name="vx-tier"]').forEach(function (i) {
      i.addEventListener("change", function () { try { localStorage.setItem("btt-bonus-tier", i.value); } catch (_) {} });
    });

    document.addEventListener("btt-theme-changed", function () { applyTod(wantedTod(now()), true); });
    document.addEventListener("btt-lang-changed", function () {
      renderAll();
      if (recordsData) renderRecords(recordsData);
      renderStatus();
      if (giveaway) renderGiveaway();
    });
  }
  var recordsData = null;

  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", boot);
  else boot();
})();

/* ===========================================================================
   class_level.js - a class's stat sheet at a chosen level (window.BTTClassLevel).

   Used by the wiki's class pages, which render partials/class_detail.html.
   ========================================================================== */
(function () {
  "use strict";

  function tr(s) { return window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s; }

  function fmtStat(s) {
    // {name, value, percentage} -> "131%" / "2,376"; null = the class has none of it
    if (s.value === null) return "—";
    var v = (typeof s.value === "number") ? s.value : 0;
    var num = (v % 1 === 0) ? v.toLocaleString() : v.toLocaleString(undefined, { maximumFractionDigits: 2 });
    return s.percentage ? num + "%" : num;
  }
  // The sheet at level N is the level-30 sheet minus every level above N.
  // Mirrors _sheet_at() in classes_page.py.
  function sheetAt(s, levels, n) {
    var v = s.value;
    if (typeof v !== "number") return v;
    Object.keys(levels || {}).forEach(function (lv) {
      if (Number(lv) <= n) return;
      levels[lv].forEach(function (x) { if (x.name === s.name) v -= x.value || 0; });
    });
    return Math.round(v * 100) / 100;
  }

  // Slider + table toggle. Works on the server-rendered DOM and on
  // renderDetail's, which share the markup.
  function wireLevel(box, c) {
    if (!box || !c) return;
    box.querySelectorAll(".cls-seg-btn").forEach(function (b) {
      b.addEventListener("click", function () {
        box.querySelectorAll(".cls-seg-btn").forEach(function (o) {
          o.setAttribute("aria-pressed", o === b ? "true" : "false");
        });
        box.querySelectorAll(".cls-growth-scroll").forEach(function (t) {
          t.hidden = t.dataset.view !== b.dataset.view;
        });
      });
    });
    var input = box.querySelector(".cls-lvl-input");
    if (!input) return;
    var out = box.querySelector(".cls-lvl-out");
    var stats = (c.stats || []).filter(function (s) { return s && s.name; });
    var vals = box.querySelectorAll(".cls-stat-val");
    var rows = box.querySelectorAll(".cls-growth-table tbody tr");
    function apply() {
      var n = Number(input.value);
      if (out) out.textContent = String(n);
      input.setAttribute("aria-valuetext", tr("Level") + " " + n);
      stats.forEach(function (s, i) {
        if (vals[i]) vals[i].textContent = fmtStat({ value: sheetAt(s, c.levels, n), percentage: s.percentage });
      });
      rows.forEach(function (r) { r.classList.toggle("is-ahead", Number(r.dataset.level) > n); });
    }
    input.addEventListener("input", apply);
    apply();
  }

  window.BTTClassLevel = { fmtStat: fmtStat, sheetAt: sheetAt, wireLevel: wireLevel };
})();

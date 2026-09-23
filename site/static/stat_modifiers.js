/* ===========================================================================
   stat_modifiers.js - the "Try it" stack on the wiki's /stat-modifiers page.
   Steps 2-5 of the game's order (docs/stat-modifiers.md), no floor/cap or late pass.
   ========================================================================== */
(function () {
  "use strict";

  const LIMIT = 1e9;
  const PRESETS = {
    knight: { base: "150", add: "6000", pct: "30", mult: "", set: "" },
    "crit-add": { base: "3000", add: "30", pct: "", mult: "", set: "" },
    "crit-pct": { base: "3000", add: "", pct: "30", mult: "", set: "" },
    pct: { base: "100", add: "", pct: "30, 30", mult: "", set: "" },
    mult: { base: "100", add: "", pct: "", mult: "1.3, 1.3", set: "" },
    zero: { base: "100", add: "50", pct: "20", mult: "0", set: "" },
  };

  const clamp = (v) => Math.max(-LIMIT, Math.min(LIMIT, v));
  const fmt = (v) => v.toLocaleString("en-US", { maximumFractionDigits: 2 });

  // "30, 15" -> [30, 15]; null when any piece isn't a number.
  function parse(input) {
    const parts = input.value.split(/[\s,]+/).filter(Boolean);
    const nums = parts.map(Number);
    const ok = nums.every(Number.isFinite);
    input.setAttribute("aria-invalid", ok ? "false" : "true");
    return ok ? nums : null;
  }

  function init() {
    const form = document.getElementById("wk-sm-form");
    const trace = document.getElementById("wk-sm-trace");
    const final = document.getElementById("wk-sm-final");
    if (!form || !trace || !final) return;
    const f = form.elements;

    function run() {
      const base = parse(f.base), add = parse(f.add), pct = parse(f.pct);
      const mult = parse(f.mult), set = parse(f.set);
      if (!base || !add || !pct || !mult || !set) {
        trace.innerHTML = '<li class="wk-sm-bad">Only numbers, separated by commas.</li>';
        final.textContent = "—";
        return;
      }
      const steps = [];
      let v = clamp(base.reduce((a, b) => a + b, 0));
      steps.push(["Base", fmt(v)]);
      if (add.length) {
        add.forEach((a) => { v = clamp(v + a); });
        steps.push(["+ flat " + add.map(fmt).join(" + "), fmt(v)]);
      }
      if (mult.length) {
        const m = mult.reduce((a, b) => a * b, 1);
        v = clamp(v * m);
        steps.push(["× multipliers " + mult.map(fmt).join(" × "), fmt(v)]);
      }
      if (pct.length) {
        const s = pct.reduce((a, b) => a + b, 0);
        v = clamp(v * (1 + s / 100));
        steps.push(["× (1 + " + pct.map((p) => fmt(p) + "%").join(" + ") + ")", fmt(v)]);
      }
      if (set.length) {
        v = Math.min(...set);
        steps.push(["Fixed value (lowest of " + set.map(fmt).join(", ") + ") replaces it", fmt(v)]);
      }
      trace.innerHTML = steps.map(([label, val]) =>
        `<li><span>${window.BTTUtil.esc(label)}</span><strong>${val}</strong></li>`).join("");
      final.textContent = fmt(v);
    }

    form.addEventListener("input", run);
    form.addEventListener("submit", (e) => e.preventDefault());
    document.querySelectorAll("[data-preset]").forEach((b) => {
      b.addEventListener("click", () => {
        const p = PRESETS[b.dataset.preset];
        Object.keys(p).forEach((k) => { f[k].value = p[k]; });
        run();
      });
    });
    run();
  }

  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", init);
  else init();
})();

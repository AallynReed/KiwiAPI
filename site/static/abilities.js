/* ═══════════════════════════════════════════════════════════════════════
   Abilities - /abilities
   ---------------------------------------------------------------------------
   All three panels (gems / rings / classes) are server-rendered, so this only
   switches between them and filters what is already in the DOM: no fetch, and
   every panel is reachable by its own ?tab= URL with JS switched off.
   ═══════════════════════════════════════════════════════════════════════ */
(function () {
  "use strict";

  var tabs = Array.prototype.slice.call(document.querySelectorAll(".ab-tab"));
  var panels = Array.prototype.slice.call(document.querySelectorAll(".ab-panel"));
  var search = document.getElementById("ab-search");
  if (!panels.length) return;

  function tr(s) { return window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s; }
  function key(el) { return el.getAttribute("data-tab") || el.id.replace("ab-tab-", ""); }

  // Per-panel filter, so switching tabs does not carry a class filter that the
  // next tab has no chip for.
  var filters = {};

  function applyPanel(panel) {
    var query = (search && search.value || "").trim().toLowerCase();
    var want = filters[key(panel)] || "";
    var cards = Array.prototype.slice.call(panel.querySelectorAll(".ab-card"));
    var shown = 0;
    cards.forEach(function (card) {
      var owned = (card.getAttribute("data-filters") || "").split("|");
      var byFilter = !want || owned.indexOf(want) !== -1;
      var byQuery = !query || (card.getAttribute("data-search") || "").indexOf(query) !== -1;
      var visible = byFilter && byQuery;
      card.hidden = !visible;
      if (visible) shown++;
    });
    var count = panel.querySelector(".ab-count");
    if (count) {
      count.textContent = shown === cards.length
        ? shown + " " + tr("abilities")
        : shown + " " + tr("of") + " " + cards.length + " " + tr("abilities");
    }
    var empty = panel.querySelector(".ab-empty");
    if (empty) empty.hidden = shown !== 0;
  }

  function apply() { panels.forEach(applyPanel); }

  function select(name, push) {
    tabs.forEach(function (tab) {
      var on = key(tab) === name;
      tab.classList.toggle("is-on", on);
      tab.setAttribute("aria-selected", String(on));
    });
    panels.forEach(function (panel) {
      var on = key(panel) === name;
      panel.classList.toggle("is-on", on);
      panel.hidden = !on;
    });
    if (push && window.history && window.history.replaceState) {
      window.history.replaceState(null, "", "?tab=" + name);
    }
  }

  tabs.forEach(function (tab) {
    tab.addEventListener("click", function (event) {
      event.preventDefault();
      select(key(tab), true);
    });
  });

  panels.forEach(function (panel) {
    var chips = Array.prototype.slice.call(panel.querySelectorAll(".ab-chip"));
    chips.forEach(function (chip) {
      chip.addEventListener("click", function () {
        filters[key(panel)] = chip.getAttribute("data-filter") || "";
        chips.forEach(function (other) {
          var on = other === chip;
          other.classList.toggle("is-on", on);
          other.setAttribute("aria-pressed", String(on));
        });
        applyPanel(panel);
      });
    });
  });

  if (search) search.addEventListener("input", apply);

  // The i18n runtime swaps copy after load; re-render the counts so they follow.
  document.addEventListener("btt-lang-changed", apply);
})();

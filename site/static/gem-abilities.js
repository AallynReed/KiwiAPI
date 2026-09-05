/* ═══════════════════════════════════════════════════════════════════════
   Gem abilities - /gem-abilities
   ---------------------------------------------------------------------------
   The list is server-rendered whole, so this only filters what is already in
   the DOM: no fetch, and the page is complete with JS switched off.
   ═══════════════════════════════════════════════════════════════════════ */
(function () {
  "use strict";

  var grid = document.getElementById("ga-grid");
  if (!grid) return;

  var cards = Array.prototype.slice.call(grid.querySelectorAll(".ga-card"));
  var search = document.getElementById("ga-search");
  var count = document.getElementById("ga-count");
  var empty = document.getElementById("ga-empty");
  var chips = Array.prototype.slice.call(document.querySelectorAll(".ga-chip"));
  var element = "";

  function tr(s) { return window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s; }

  function apply() {
    var query = (search && search.value || "").trim().toLowerCase();
    var shown = 0;
    cards.forEach(function (card) {
      var elements = (card.getAttribute("data-elements") || "").split(",");
      var byElement = !element || elements.indexOf(element) !== -1;
      var byQuery = !query || (card.getAttribute("data-search") || "").indexOf(query) !== -1;
      var visible = byElement && byQuery;
      card.hidden = !visible;
      if (visible) shown++;
    });
    if (count) {
      count.textContent = shown === cards.length
        ? shown + " " + tr("abilities")
        : shown + " " + tr("of") + " " + cards.length + " " + tr("abilities");
    }
    if (empty) empty.hidden = shown !== 0;
  }

  chips.forEach(function (chip) {
    chip.addEventListener("click", function () {
      element = chip.getAttribute("data-element") || "";
      chips.forEach(function (other) {
        var on = other === chip;
        other.classList.toggle("is-on", on);
        other.setAttribute("aria-pressed", String(on));
      });
      apply();
    });
  });

  if (search) search.addEventListener("input", apply);

  // The i18n runtime swaps copy after load; re-render the count so it follows.
  document.addEventListener("btt-lang-changed", apply);
})();

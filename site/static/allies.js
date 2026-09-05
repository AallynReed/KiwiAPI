/* ═══════════════════════════════════════════════════════════════════════
   Allies table - /allies
   ---------------------------------------------------------------------------
   Every row is server-rendered and name-sorted, so the table is complete and
   useful without JS. This only sorts and filters in place: picking a stat sorts
   by it descending and drops the allies that do not grant it, which is the
   question the table exists to answer.
   ═══════════════════════════════════════════════════════════════════════ */
(function () {
  "use strict";

  var body = document.getElementById("al-body");
  if (!body) return;

  var rows = Array.prototype.slice.call(body.querySelectorAll(".al-row"));
  var search = document.getElementById("al-search");
  var sort = document.getElementById("al-sort");
  var count = document.getElementById("al-count");
  var empty = document.getElementById("al-empty");
  var header = document.getElementById("al-sorted-col");
  // Name order as rendered, so clearing the sort restores it exactly.
  var original = rows.slice();

  function tr(s) { return window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s; }
  function valueOf(row, slug) {
    var raw = row.getAttribute("data-s-" + slug);
    return raw === null ? null : parseFloat(raw);
  }

  function apply() {
    var query = (search && search.value || "").trim().toLowerCase();
    var slug = (sort && sort.value) || "";
    var label = slug && sort.options[sort.selectedIndex].textContent;

    var shown = 0;
    rows.forEach(function (row) {
      var value = slug ? valueOf(row, slug) : null;
      // Sorting by a stat is also a filter: an ally without it has no place in a
      // ranking of it, and leaving them in pushes 1,000 blanks below the answer.
      var byStat = !slug || value !== null;
      var byQuery = !query || (row.getAttribute("data-search") || "").indexOf(query) !== -1;
      var visible = byStat && byQuery;
      row.hidden = !visible;
      if (visible) shown++;

      var cell = row.querySelector(".al-sorted");
      if (cell) {
        cell.hidden = !slug;
        cell.textContent = slug && value !== null ? String(value) : "";
      }
    });

    if (header) {
      header.hidden = !slug;
      header.textContent = label || "";
    }

    var order = slug
      ? rows.slice().sort(function (a, b) {
          var d = (valueOf(b, slug) || 0) - (valueOf(a, slug) || 0);
          return d || a.querySelector(".al-name").textContent
            .localeCompare(b.querySelector(".al-name").textContent);
        })
      : original;
    // One reflow rather than one per row.
    var frag = document.createDocumentFragment();
    order.forEach(function (row) { frag.appendChild(row); });
    body.appendChild(frag);

    if (count) {
      count.textContent = shown === rows.length
        ? shown + " " + tr("allies")
        : shown + " " + tr("of") + " " + rows.length + " " + tr("allies");
    }
    if (empty) empty.hidden = shown !== 0;
  }

  if (search) search.addEventListener("input", apply);
  if (sort) sort.addEventListener("change", apply);
  document.addEventListener("btt-lang-changed", apply);
})();

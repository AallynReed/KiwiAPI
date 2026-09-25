/* Wiki list filter (/allies, /fish, /recipes…): narrows the server-rendered list in
   place by text, by any facet <select data-filter-facet="x"> (matched against each
   item's data-x) and optionally to entries that have abilities. */
(function () {
  "use strict";
  const root = document.querySelector("[data-filter-root]");
  const list = document.querySelector("[data-filter-list]");
  if (!root || !list) return;
  const input = root.querySelector("[data-filter-input]");
  const onlyAbilities = root.querySelector("[data-filter-abilities]");
  const facets = Array.from(root.querySelectorAll("[data-filter-facet]"));
  const count = root.querySelector("[data-filter-count]");
  const empty = document.querySelector("[data-filter-empty]");
  const items = Array.from(list.querySelectorAll("[data-search]"));
  const groups = Array.from(list.querySelectorAll("[data-filter-group]"));

  function apply() {
    const terms = input.value.trim().toLowerCase().split(/\s+/).filter(Boolean);
    const needAbility = onlyAbilities && onlyAbilities.checked;
    const wanted = facets.map((f) => [f.dataset.filterFacet, f.value]).filter(([, v]) => v);
    let shown = 0;
    for (const li of items) {
      const hay = li.dataset.search || "";
      const ok = terms.every((t) => hay.includes(t))
        && (!needAbility || li.dataset.abilities !== "0")
        && wanted.every(([k, v]) => (li.dataset[k] || "").split("|").includes(v));
      li.hidden = !ok;
      if (ok) shown++;
    }
    for (const g of groups) {
      const visible = g.querySelectorAll("[data-search]:not([hidden])").length;
      g.hidden = !visible;
      const n = g.querySelector("[data-group-count]");
      if (n) n.textContent = visible;
    }
    count.textContent = shown + " shown";
    if (empty) empty.hidden = shown !== 0;
  }

  input.addEventListener("input", apply);
  if (onlyAbilities) onlyAbilities.addEventListener("change", apply);
  facets.forEach((f) => f.addEventListener("change", apply));
  const params = new URLSearchParams(location.search);
  if (params.get("q")) input.value = params.get("q");
  for (const f of facets) {
    const v = params.get(f.dataset.filterFacet);
    if (v && Array.from(f.options).some((o) => o.value === v)) f.value = v;
  }
  apply();
})();

/* Wiki list filter (/allies, /mounts): narrows the server-rendered list in place,
   by name/description text and optionally to entries that have abilities. */
(function () {
  "use strict";
  const root = document.querySelector("[data-filter-root]");
  const list = document.querySelector("[data-filter-list]");
  if (!root || !list) return;
  const input = root.querySelector("[data-filter-input]");
  const onlyAbilities = root.querySelector("[data-filter-abilities]");
  const count = root.querySelector("[data-filter-count]");
  const empty = document.querySelector("[data-filter-empty]");
  const items = Array.from(list.children);

  function apply() {
    const terms = input.value.trim().toLowerCase().split(/\s+/).filter(Boolean);
    const needAbility = onlyAbilities.checked;
    let shown = 0;
    for (const li of items) {
      const hay = li.dataset.search || "";
      const ok = terms.every((t) => hay.includes(t)) && (!needAbility || li.dataset.abilities !== "0");
      li.hidden = !ok;
      if (ok) shown++;
    }
    count.textContent = shown + " shown";
    if (empty) empty.hidden = shown !== 0;
  }

  input.addEventListener("input", apply);
  onlyAbilities.addEventListener("change", apply);
  const q = new URLSearchParams(location.search).get("q");
  if (q) { input.value = q; }
  apply();
})();

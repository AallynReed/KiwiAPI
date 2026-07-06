/* ===========================================================================
   classes.js - the Trove class reference (/classes).

   Fetches every class once from the same-origin /site/stats/classes proxy,
   renders a picker, and shows the selected class's stats / subclass / abilities.
   Deep-links via the URL hash (#knight), so a class page is shareable.
   ========================================================================== */
(function () {
  "use strict";

  const { getJSON } = window.BTTUtil;
  function tr(s) { return window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s; }
  function el(tag, cls, txt) {
    var e = document.createElement(tag);
    if (cls) e.className = cls;
    if (txt != null) e.textContent = txt;
    return e;
  }
  function fmtStat(s) {
    // {name, value, percentage} -> "131%" / "2,376"
    var v = (typeof s.value === "number") ? s.value : 0;
    var num = (v % 1 === 0) ? v.toLocaleString() : v.toLocaleString(undefined, { maximumFractionDigits: 2 });
    return s.percentage ? num + "%" : num;
  }
  function dmgClass(d) { return (d || "").toLowerCase() === "physical" ? "physical" : "magic"; }
  // Some classes carry placeholder subclass rows ({name:"", value:0}); a bonus is
  // only worth showing if it has a real stat name or a non-zero value.
  function meaningful(list) {
    return (list || []).filter(function (b) { return b && ((b.name && b.name.trim()) || b.value); });
  }
  function iconUrl(tech) { return "/static/class-icons/" + encodeURIComponent(tech || "") + ".png"; }

  var CLASSES = [], byTech = {};

  getJSON("/site/stats/classes").then(function (d) {
    CLASSES = (d && d.items) || [];
    CLASSES.forEach(function (c) { byTech[c.tech_name] = c; });
    if (!CLASSES.length) { setEmpty(); return; }
    buildNav();
    var initial = (location.hash || "").replace(/^#/, "");
    select(byTech[initial] ? initial : CLASSES[0].tech_name, false);
  }).catch(function () { setEmpty(); });

  function setEmpty() {
    var nav = document.getElementById("cls-nav"), det = document.getElementById("cls-detail");
    if (nav) { nav.textContent = ""; nav.appendChild(el("p", "cls-empty", tr("Couldn't load classes."))); }
    if (det) det.textContent = "";
  }

  function buildNav() {
    var nav = document.getElementById("cls-nav");
    if (!nav) return;
    nav.textContent = "";
    CLASSES.forEach(function (c) {
      var btn = el("button", "cls-navitem");
      btn.type = "button";
      btn.dataset.tech = c.tech_name;
      var img = document.createElement("img");
      img.src = iconUrl(c.tech_name); img.alt = ""; img.loading = "lazy";
      img.onerror = function () { img.style.visibility = "hidden"; };
      btn.appendChild(img);
      btn.appendChild(el("span", "cls-navitem-name", c.name));
      btn.appendChild(el("span", "cls-navitem-dmg dmg-" + dmgClass(c.damage_type), c.damage_type || ""));
      btn.addEventListener("click", function () { select(c.tech_name, true); });
      nav.appendChild(btn);
    });
  }

  function select(tech, updateHash) {
    var c = byTech[tech];
    if (!c) return;
    document.querySelectorAll(".cls-navitem").forEach(function (b) {
      var on = b.dataset.tech === tech;
      b.classList.toggle("active", on);
      if (on) b.setAttribute("aria-current", "true"); else b.removeAttribute("aria-current");
    });
    if (updateHash && location.hash.replace(/^#/, "") !== tech) {
      history.replaceState(null, "", "#" + tech);
    }
    renderDetail(c);
  }

  function renderDetail(c) {
    var box = document.getElementById("cls-detail");
    if (!box) return;
    box.textContent = "";

    // Head
    var head = el("div", "cls-detail-head");
    var icon = document.createElement("img");
    icon.className = "cls-detail-icon"; icon.src = iconUrl(c.tech_name); icon.alt = c.name || "";
    icon.onerror = function () { icon.style.visibility = "hidden"; };
    head.appendChild(icon);
    var title = el("div", "cls-detail-title");
    title.appendChild(el("h2", "cls-detail-name", c.name || ""));
    var tags = el("div", "cls-detail-tags");
    if (c.damage_type) {
      var dt = el("span", "cls-tag is-" + dmgClass(c.damage_type));
      dt.appendChild(el("i", "fa-solid " + (dmgClass(c.damage_type) === "magic" ? "fa-wand-sparkles" : "fa-hand-fist")));
      dt.appendChild(el("span", null, c.damage_type + " " + tr("damage")));
      tags.appendChild(dt);
    }
    (c.weapons || []).forEach(function (w) {
      var t = el("span", "cls-tag");
      t.appendChild(el("i", "fa-solid fa-khanda"));
      t.appendChild(el("span", null, w));
      tags.appendChild(t);
    });
    if (c.shorts && c.shorts.length) {
      var s = el("span", "cls-tag");
      s.appendChild(el("i", "fa-solid fa-tag"));
      s.appendChild(el("span", null, c.shorts.join(" / ")));
      tags.appendChild(s);
    }
    title.appendChild(tags);
    head.appendChild(title);
    box.appendChild(head);

    // Base stats
    var stats = (c.stats || []).filter(function (s) { return s && s.name; });
    if (stats.length) {
      box.appendChild(section(tr("Base stats"), "fa-chart-simple", (function () {
        var grid = el("div", "cls-stats");
        stats.forEach(function (s) {
          var row = el("div", "cls-stat");
          row.appendChild(el("span", "cls-stat-name", s.name));
          row.appendChild(el("span", "cls-stat-val", fmtStat(s)));
          grid.appendChild(row);
        });
        return grid;
      })()));
    }

    // Class gem / bonus stats (non-zero only)
    var bonuses = (c.bonuses || []).filter(function (b) { return b && b.name && b.value; });
    if (bonuses.length) {
      box.appendChild(section(tr("Class bonuses"), "fa-plus", (function () {
        var chips = el("div", "cls-chips");
        bonuses.forEach(function (b) {
          chips.appendChild(el("span", "cls-chip", b.name + " +" + fmtStat(b)));
        });
        return chips;
      })()));
    }

    // Subclass. Some classes ship placeholder-only subclass data (no name/desc,
    // all-zero level rows) - only render the section when there's real content.
    var sc = c.subclass || {};
    var lv = sc.level || {};
    var scTiers = Object.keys(lv).sort(function (a, b) { return Number(a) - Number(b); })
      .map(function (tier) { return { tier: tier, bonuses: meaningful(lv[tier]) }; })
      .filter(function (t) { return t.bonuses.length; });
    if (sc.name || sc.description || scTiers.length) {
      box.appendChild(section(tr("Subclass"), "fa-star", (function () {
        var wrap = el("div", "cls-subclass");
        if (sc.name) wrap.appendChild(el("div", "cls-subclass-name", sc.name));
        if (sc.description) wrap.appendChild(el("div", "cls-subclass-desc", sc.description));
        if (scTiers.length) {
          var grid = el("div", "cls-levels");
          scTiers.forEach(function (t) {
            var cell = el("div", "cls-level");
            cell.appendChild(el("div", "cls-level-tier", tr("Level") + " " + t.tier));
            t.bonuses.forEach(function (b) {
              var line = el("div", "cls-level-bonus");
              if (b.name && b.name.trim()) line.appendChild(document.createTextNode(b.name + " "));
              line.appendChild(el("strong", null, "+" + fmtStat(b)));
              cell.appendChild(line);
            });
            grid.appendChild(cell);
          });
          wrap.appendChild(grid);
        }
        return wrap;
      })()));
    }

    // Abilities (if the class data carries any)
    var abilities = (c.abilities || []).filter(function (a) { return a && (a.name || a.description); });
    if (abilities.length) {
      box.appendChild(section(tr("Abilities"), "fa-bolt", (function () {
        var list = el("div", "cls-abilities");
        abilities.forEach(function (a) {
          var card = el("div", "cls-ability");
          if (a.name) card.appendChild(el("div", "cls-ability-name", a.name));
          if (a.description) card.appendChild(el("div", "cls-ability-desc", a.description));
          list.appendChild(card);
        });
        return list;
      })()));
    }
  }

  function section(titleText, icon, node) {
    var sec = el("section", "cls-section");
    var h = el("h3", "cls-section-title");
    h.appendChild(el("i", "fa-solid " + icon));
    h.appendChild(el("span", null, titleText));
    sec.appendChild(h);
    sec.appendChild(node);
    return sec;
  }

  // React to back/forward navigation between deep-linked classes.
  window.addEventListener("hashchange", function () {
    var tech = (location.hash || "").replace(/^#/, "");
    if (byTech[tech]) select(tech, false);
  });
})();

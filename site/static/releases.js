/* ===========================================================================
   releases.js - the Better Trove Tools app releases + changelog (/releases).

     latest per platform -> /site/btt/latest?channel=
     release history     -> /site/btt/releases?channel=
     changelog           -> /site/btt/changelog   (channel-independent)

   Fails closed: any section that errors shows an empty state.
   ========================================================================== */
(function () {
  "use strict";

  const { getJSON } = window.BTTUtil;
  function el(tag, cls, txt) {
    var e = document.createElement(tag);
    if (cls) e.className = cls;
    if (txt != null) e.textContent = txt;
    return e;
  }
  function fmtBytes(n) {
    if (!n) return "";
    if (n >= 1048576) return (n / 1048576).toFixed(1) + " MB";
    if (n >= 1024) return (n / 1024).toFixed(0) + " KB";
    return n + " B";
  }
  function fmtDate(v) {
    if (!v) return "";
    var t = (typeof v === "number") ? v * 1000 : Date.parse(v);
    if (!t) return "";
    return new Date(t).toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" });
  }
  function renderMd(text) {
    if (window.BTTMarkdown && text) { try { return window.BTTMarkdown.render(text); } catch (e) {} }
    return null;
  }

  var PLATFORMS = [
    { key: "windows", label: "Windows", icon: "fa-brands fa-windows" },
    { key: "linux", label: "Linux", icon: "fa-brands fa-linux" },
    { key: "android", label: "Android", icon: "fa-brands fa-android" },
  ];

  var channel = "release";

  /* ---- Latest per platform ---------------------------------------------- */
  function loadLatest() {
    var box = document.getElementById("rel-latest");
    if (!box) return;
    box.textContent = "";
    box.appendChild(el("p", "rel-loading", "Loading downloads…"));
    getJSON("/site/btt/latest?channel=" + channel).then(function (d) {
      box.textContent = "";
      var plats = d.platforms || {};
      PLATFORMS.forEach(function (p) {
        box.appendChild(platCard(p, plats[p.key]));
      });
    }).catch(function () {
      box.textContent = "";
      box.appendChild(el("p", "rel-empty", "Couldn't load downloads."));
    });
  }

  function platCard(p, info) {
    var card = el("div", "rel-plat");
    var top = el("div", "rel-plat-top");
    var ic = el("span", "rel-plat-ic"); ic.appendChild(el("i", p.icon)); top.appendChild(ic);
    var head = el("div");
    head.appendChild(el("div", "rel-plat-name", p.label));
    if (info && info.tag_name) {
      head.appendChild(el("div", "rel-plat-ver", info.tag_name + " · " + fmtDate(info.published_at)));
    } else {
      head.appendChild(el("div", "rel-plat-ver", "No build on this channel"));
    }
    top.appendChild(head);
    card.appendChild(top);

    var assets = (info && info.assets) || [];
    if (!assets.length) {
      card.appendChild(el("p", "rel-plat-empty", info ? "No downloadable file." : "Not available yet."));
      return card;
    }
    var list = el("div", "rel-plat-assets");
    assets.forEach(function (a, i) {
      var link = el("a", "rel-dl" + (i === 0 ? " rel-dl-primary" : ""));
      link.href = a.url; link.rel = "noopener";
      link.appendChild(el("i", "fa-solid fa-download"));
      var body = el("div", "rel-dl-body");
      body.appendChild(el("div", "rel-dl-name", a.name));
      var meta = [];
      if (a.size) meta.push(fmtBytes(a.size));
      if (typeof a.download_count === "number") meta.push(a.download_count.toLocaleString() + " downloads");
      body.appendChild(el("div", "rel-dl-meta", meta.join(" · ")));
      link.appendChild(body);
      list.appendChild(link);
    });
    card.appendChild(list);
    return card;
  }

  /* ---- Release history --------------------------------------------------- */
  function loadReleases() {
    var box = document.getElementById("rel-releases");
    if (!box) return;
    box.textContent = "";
    box.appendChild(el("p", "rel-loading", "Loading releases…"));
    getJSON("/site/btt/releases?channel=" + channel + "&limit=30").then(function (d) {
      box.textContent = "";
      var items = d.items || [];
      if (!items.length) { box.appendChild(el("p", "rel-empty", "No releases on this channel yet.")); return; }
      items.forEach(function (r, i) { box.appendChild(releaseRow(r, i === 0)); });
    }).catch(function () {
      box.textContent = "";
      box.appendChild(el("p", "rel-empty", "Couldn't load releases."));
    });
  }

  function releaseRow(r, open) {
    var det = document.createElement("details");
    det.className = "rel-release";
    if (open) det.open = true;
    var sum = document.createElement("summary");
    sum.className = "rel-release-sum";
    sum.appendChild(el("span", "rel-release-tag", r.tag_name || ""));
    var beta = r.channel === "beta";
    sum.appendChild(el("span", "rel-badge " + (beta ? "rel-badge-beta" : "rel-badge-release"), beta ? "Beta" : "Stable"));
    sum.appendChild(el("span", "rel-release-date", fmtDate(r.published_at)));
    var caret = el("i", "fa-solid fa-chevron-down rel-release-caret"); sum.appendChild(caret);
    det.appendChild(sum);

    var body = el("div", "rel-release-body");
    if (r.name && r.name !== r.tag_name) body.appendChild(el("div", "rel-release-name", r.name));
    var html = renderMd(r.body);
    if (html) { var notes = el("div", "rel-notes"); notes.innerHTML = html; body.appendChild(notes); }
    else if (r.body) { var pre = el("div", "rel-notes"); pre.textContent = r.body; body.appendChild(pre); }
    else body.appendChild(el("p", "rel-notes-empty", "No release notes."));

    var assets = r.assets || [];
    if (assets.length) {
      var av = el("div", "rel-release-assets");
      assets.forEach(function (a) {
        var link = el("a", "rel-asset"); link.href = a.url; link.rel = "noopener";
        link.appendChild(el("i", "fa-solid fa-download"));
        link.appendChild(el("span", null, a.name + (a.size ? " (" + fmtBytes(a.size) + ")" : "")));
        av.appendChild(link);
      });
      body.appendChild(av);
    }
    det.appendChild(body);
    return det;
  }

  /* ---- Changelog (channel-independent) ----------------------------------- */
  (function loadChangelog() {
    var box = document.getElementById("rel-changelog");
    if (!box) return;
    getJSON("/site/btt/changelog").then(function (d) {
      box.textContent = "";
      var groups = d.groups || [];
      if (!groups.length) { box.appendChild(el("p", "rel-empty", "No changelog available.")); return; }
      if (d.rate_limited) box.appendChild(el("p", "rel-cl-note", "GitHub rate-limited the last refresh - showing the most recent cached changelog."));
      groups.slice(0, 12).forEach(function (g) { box.appendChild(clGroup(g)); });
    }).catch(function () {
      box.textContent = "";
      box.appendChild(el("p", "rel-empty", "Couldn't load changelog."));
    });
  })();

  function clGroup(g) {
    var wrap = el("div", "rel-cl-group");
    var unreleased = (g.version || "").toLowerCase() === "unreleased";
    var ver = el("div", "rel-cl-ver" + (unreleased ? " is-unreleased" : ""));
    ver.appendChild(el("i", "fa-solid " + (unreleased ? "fa-code-commit" : "fa-tag")));
    ver.appendChild(el("span", null, g.version || ""));
    wrap.appendChild(ver);
    var list = el("div", "rel-cl-commits");
    (g.commits || []).slice(0, 30).forEach(function (c) {
      var row = el("div", "rel-cl-commit");
      var type = (c.type || "").toLowerCase();
      row.appendChild(el("span", "rel-cl-type t-" + type, type || "·"));
      var msg = el("span", "rel-cl-msg");
      if (c.url) {
        var a = el("a", null, c.message || ""); a.href = c.url; a.target = "_blank"; a.rel = "noopener";
        a.title = c.short_sha || "";
        msg.appendChild(a);
      } else {
        msg.textContent = c.message || "";
      }
      row.appendChild(msg);
      list.appendChild(row);
    });
    wrap.appendChild(list);
    return wrap;
  }

  /* ---- Channel toggle ---------------------------------------------------- */
  var toggle = document.querySelector(".rel-channel");
  if (toggle) toggle.addEventListener("click", function (e) {
    var btn = e.target.closest(".rel-channel-btn");
    if (!btn || btn.dataset.channel === channel) return;
    channel = btn.dataset.channel;
    toggle.querySelectorAll(".rel-channel-btn").forEach(function (b) {
      var on = b === btn;
      b.classList.toggle("active", on);
      b.setAttribute("aria-selected", on ? "true" : "false");
    });
    loadLatest();
    loadReleases();
  });

  loadLatest();
  loadReleases();
})();

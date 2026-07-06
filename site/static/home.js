/* ===========================================================================
   home.js - the homepage dashboard (/).

   Fetches every section same-origin and renders it; all of it fails closed
   (a section that errors just shows an empty/▸loading state). Sources:
     clock + Today in Trove -> /site/rotations
     official news          -> /site/feeds/news
     community videos       -> /site/feeds/videos?platform=
     players active         -> /site/leaderboards/activity
     server status          -> /site/trove-status
     latest update          -> /site/updates/live-us/versions
     latest mods            -> /site/mods/projects
     giveaways              -> /site/giveaways
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
  // Biome pill with its icon. Accepts {name, icon} (current) or a bare string
  // (tolerated during a frontend-before-backend deploy window).
  function biomePill(b) {
    var name = (typeof b === "string") ? b : ((b && b.name) || "");
    var icon = (b && typeof b === "object") ? b.icon : null;
    var span = el("span", "dash-biome-pill");
    if (icon) {
      var img = document.createElement("img");
      img.src = "/static/assets/biomes/" + icon + ".png";
      img.alt = ""; img.loading = "lazy";
      img.onerror = function () { img.style.display = "none"; };
      span.appendChild(img);
    }
    span.appendChild(el("span", null, name));
    return span;
  }
  function pad(n) { return n < 10 ? "0" + n : "" + n; }
  function fmtIn(sec) {
    if (sec == null) return "—";
    if (sec <= 0) return tr("now");
    var d = Math.floor(sec / 86400), h = Math.floor((sec % 86400) / 3600), m = Math.floor((sec % 3600) / 60);
    if (d > 0) return d + "d " + h + "h";
    if (h > 0) return h + "h " + m + "m";
    return m + "m";
  }
  function toUnix(v) {
    if (v == null) return null;
    if (typeof v === "number") return v;
    var t = Date.parse(v);
    return isNaN(t) ? null : Math.floor(t / 1000);
  }
  function timeAgo(ts) {
    var t = typeof ts === "number" ? ts * 1000 : Date.parse(ts);
    if (!t) return "";
    var s = Math.max(0, (Date.now() - t) / 1000);
    if (s < 3600) return Math.floor(s / 60) + "m " + tr("ago");
    if (s < 86400) return Math.floor(s / 3600) + "h " + tr("ago");
    return Math.floor(s / 86400) + "d " + tr("ago");
  }

  function fmtDate(unix) {
    if (!unix) return "";
    var d = new Date(unix * 1000);
    return d.toLocaleDateString(undefined, { month: "short", day: "numeric" }) + " " +
      d.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
  }

  /* ---- Shared detail modal ----------------------------------------------- */
  var modalEl = document.getElementById("dash-modal");
  var modalHead = document.getElementById("dash-modal-head");
  var modalBody = document.getElementById("dash-modal-body");
  function closeModal() { if (modalEl) { modalEl.hidden = true; document.body.style.overflow = ""; } }
  function openModal(headSetup, bodyNode) {
    if (!modalEl) return;
    modalHead.textContent = ""; modalBody.textContent = "";
    modalHead.removeAttribute("style");
    headSetup(modalHead);
    if (bodyNode) modalBody.appendChild(bodyNode);
    modalEl.hidden = false;
    document.body.style.overflow = "hidden";
  }
  if (modalEl) {
    modalEl.addEventListener("click", function (e) { if (e.target.closest("[data-close]")) closeModal(); });
    document.addEventListener("keydown", function (e) { if (e.key === "Escape") closeModal(); });
  }
  function clickable(node, fn) {
    node.setAttribute("role", "button");
    node.tabIndex = 0;
    node.addEventListener("click", fn);
    node.addEventListener("keydown", function (e) { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); fn(); } });
    return node;
  }

  /* ---- Player lookup ----------------------------------------------------- */
  var form = document.getElementById("home-player-search");
  if (form) form.addEventListener("submit", function (e) {
    e.preventDefault();
    var v = (document.getElementById("home-player-input").value || "").trim();
    if (v) window.location.href = "/player/" + encodeURIComponent(v);
  });

  /* ---- Today in Trove + clock (/site/rotations) -------------------------- */
  (function () {
    // The buff/merchant cards embed a live countdown snapshot as static text
    // (they recompute on click via the modal, not per-second), so they must be
    // re-rendered on a language switch. The ticking clock loop is registered
    // ONCE (guarded below) and reads its own live values, so it is never
    // re-registered — that would stack duplicate 1s loops.
    var rotationsData = null, tickStarted = false;

    function renderRotations(d) {
      var st = d.server_time || {};
      var anchor = (typeof st.now_unix === "number") ? st.now_unix : Math.floor(Date.now() / 1000);
      var t0 = Date.now();
      function nowU() { return Math.floor(anchor + (Date.now() - t0) / 1000); }

      // --- ticking clock + resets (registered once, never on re-render) ---
      if (!tickStarted) {
        tickStarted = true;
        var timeEl = document.getElementById("dash-clock-time");
        var dEl = document.getElementById("dash-daily"), wEl = document.getElementById("dash-weekly");
        (function tick() {
          var n = nowU();
          var t = new Date((n - 11 * 3600) * 1000);
          if (timeEl) timeEl.textContent = pad(t.getUTCHours()) + ":" + pad(t.getUTCMinutes()) + ":" + pad(t.getUTCSeconds());
          if (dEl && st.daily_reset_at) dEl.textContent = tr("in") + " " + fmtIn(st.daily_reset_at - n);
          if (wEl && st.weekly_reset_at) wEl.textContent = tr("in") + " " + fmtIn(st.weekly_reset_at - n);
          setTimeout(tick, 1000);
        })();
      }

      // --- buff + chaos cards (click -> modal) ---
      var buffs = document.getElementById("dash-buffs");
      if (buffs) {
        buffs.textContent = "";
        var db = buffCard(tr("Today's bonus"), d.daily_buff, d.daily_rotation, tr("Daily bonus rotation"));
        var wb = buffCard(tr("This week's bonus"), d.weekly_buff, d.weekly_rotation, tr("Weekly bonus rotation"));
        if (db) buffs.appendChild(db);
        if (wb) buffs.appendChild(wb);
        if (d.chaos && d.chaos.ends_at) buffs.appendChild(chaosCard(d.chaos, nowU));
        if (!buffs.children.length) buffs.appendChild(el("p", "dash-empty", "—"));
      }

      // --- merchants (click -> schedule modal) ---
      var mEl = document.getElementById("dash-merchants");
      if (mEl && d.merchants) {
        mEl.textContent = "";
        d.merchants.forEach(function (m) { mEl.appendChild(merchantCard(m, nowU)); });
      }

      function buffCard(kicker, buff, rotation, title) {
        if (!buff) return null;
        var card = el("div", "dash-buff is-clickable");
        if (buff.color) card.style.setProperty("--buff-accent", "#" + buff.color);
        var head = el("div", "dash-buff-head");
        if (buff.banner) head.style.backgroundImage = "linear-gradient(to right, rgba(6,8,13,.85), rgba(6,8,13,.35)), url('" + buff.banner + "')";
        head.appendChild(el("span", null, (buff.emoji ? buff.emoji + " " : "") + (buff.name || "")));
        card.appendChild(head);
        card.appendChild(el("span", "dash-buff-kicker", kicker));
        var list = el("ul", "dash-buff-list");
        (buff.normal_buffs || buff.buffs || []).slice(0, 4).forEach(function (b) { list.appendChild(el("li", null, b)); });
        card.appendChild(list);
        return clickable(card, function () { rotationModal(title, rotation || [], nowU()); });
      }
      function chaosCard(chaos, now) {
        var card = el("div", "dash-buff dash-buff-chaos is-clickable");
        card.appendChild(el("span", "dash-buff-kicker", tr("Chaos Chest")));
        var item = chaos.item;
        card.appendChild(el("div", "dash-chaos-time", (item && item.name) ? item.name : tr("Featured item")));
        card.appendChild(el("div", "dash-chaos-sub", tr("Resets in") + " " + fmtIn(chaos.ends_at - now())));
        return clickable(card, function () { chaosModal(chaos, now()); });
      }
      function merchantCard(m, now) {
        var n = now();
        var card = el("div", "dash-merchant is-clickable" + (m.active ? " is-active" : " is-inactive"));
        var top = el("div", "dash-merchant-top");
        top.appendChild(el("span", "dash-merchant-name", m.name));
        top.appendChild(el("span", "dash-merchant-badge", m.active ? (m.state || tr("Here")) : tr("Away")));
        card.appendChild(top);
        if (m.ends_at) {
          card.appendChild(el("div", "dash-merchant-time",
            (m.active ? tr("Leaves in") : tr("Returns in")) + " " + fmtIn((m.active ? m.ends_at : m.starts_at) - n)));
        }
        if (m.biomes && m.biomes.length) {
          var bs = el("div", "dash-merchant-biomes");
          m.biomes.slice(0, 3).forEach(function (b) { bs.appendChild(biomePill(b)); });
          card.appendChild(bs);
        }
        return clickable(card, function () { merchantModal(m, now()); });
      }
    }

    getJSON("/site/rotations").then(function (d) {
      rotationsData = d;
      renderRotations(d);
    }).catch(function () {});

    document.addEventListener("btt-lang-changed", function () {
      if (rotationsData) renderRotations(rotationsData);
    });

    // --- modal renderers (hoisted) ---
    // Full daily (7-day) / weekly (4-week) bonus rotation: every entry in
    // rotation order, the active one highlighted, each with when it's next up.
    function rotationModal(title, entries, n) {
      var body = el("div");
      if (!entries.length) {
        body.appendChild(el("p", "dash-modal-note", tr("Rotation unavailable right now.")));
      } else {
        var ul = el("ul", "dash-rot");
        entries.forEach(function (e) {
          var li = el("li", "dash-rot-row" + (e.is_current ? " is-now" : ""));
          if (e.color) li.style.setProperty("--buff-accent", "#" + e.color);
          var top = el("div", "dash-rot-top");
          var name = el("div", "dash-rot-name");
          if (e.emoji) name.appendChild(el("span", "dash-rot-emoji", e.emoji));
          name.appendChild(el("span", null, e.name || ""));
          if (e.weekday) name.appendChild(el("span", "dash-rot-day", e.weekday));
          top.appendChild(name);
          top.appendChild(el("span", "dash-rot-when" + (e.is_current ? " is-now" : ""),
            e.is_current ? tr("Active now") : tr("in") + " " + fmtIn((e.next_at || 0) - n)));
          li.appendChild(top);
          var bl = e.normal_buffs || e.buffs || [];
          if (bl.length) li.appendChild(el("div", "dash-rot-buffs", bl.join(" · ")));
          ul.appendChild(li);
        });
        body.appendChild(ul);
      }
      openModal(function (head) {
        head.style.color = "var(--text)"; head.style.textShadow = "none";
        head.textContent = title;
      }, body);
    }
    function chaosModal(chaos, n) {
      var body = el("div");
      var item = chaos.item;
      if (item && item.name) {
        body.appendChild(el("p", "dash-modal-note", tr("Featured item this week")));
        body.appendChild(el("p", "dash-chaos-modal-item", item.name));
      } else {
        body.appendChild(el("p", "dash-modal-note", tr("The featured item rotates every week (not captured yet).")));
      }
      body.appendChild(el("p", null, tr("Window:") + " " + fmtDate(chaos.starts_at) + " → " + fmtDate(chaos.ends_at)));
      if (chaos.ends_at) body.appendChild(el("p", null, tr("Resets in") + " " + fmtIn(chaos.ends_at - n)));
      openModal(function (head) { head.textContent = "🎁 " + tr("Chaos Chest"); }, body);
    }
    function merchantModal(m, n) {
      var body = el("div");
      var status = m.active
        ? tr("Here now") + (m.ends_at ? " · " + tr("leaves in") + " " + fmtIn(m.ends_at - n) : "")
        : (m.starts_at ? tr("Returns in") + " " + fmtIn(m.starts_at - n) : tr("Away"));
      body.appendChild(el("p", "dash-modal-note", status));
      var sched = m.schedule || [];
      if (!sched.length) {
        body.appendChild(el("p", "dash-modal-note", tr("No upcoming schedule available.")));
      } else {
        var th = el("div", "dash-modal-col-title");
        th.appendChild(el("i", "fa-regular fa-calendar")); th.appendChild(el("span", null, tr("Upcoming")));
        body.appendChild(th);
        var ul = el("ul", "dash-sched");
        sched.forEach(function (s) {
          var isNow = s.starts_at <= n && s.ends_at > n;
          var li = el("li", "dash-sched-row" + (isNow ? " is-now" : ""));
          var time = el("div", "dash-sched-time");
          time.appendChild(el("span", null, fmtDate(s.starts_at) + " – " + fmtDate(s.ends_at)));
          if (isNow) time.appendChild(el("span", "dash-sched-now", tr("Now")));
          else if (s.state) time.appendChild(el("span", "dash-sched-state", s.state));
          li.appendChild(time);
          if (s.biomes && s.biomes.length) {
            var bs = el("div", "dash-sched-biomes");
            s.biomes.forEach(function (x) { bs.appendChild(biomePill(x)); });
            li.appendChild(bs);
          }
          ul.appendChild(li);
        });
        body.appendChild(ul);
      }
      openModal(function (head) {
        head.style.color = "var(--text)"; head.style.textShadow = "none";
        head.textContent = m.name;
      }, body);
    }
  })();

  /* ---- Record highs (/site/leaderboards/records) ------------------------- */
  // The all-time ceiling of Trove Mastery, Geode Mastery and Power Rank, read
  // off the rank-1 holder of each lifetime board. Mastery arrives as points +
  // a resolved level; Power Rank is a bare number. Fails closed (empty state).
  (function () {
    var box = document.getElementById("dash-records");
    if (!box) return;
    function num(n) { return (n == null) ? "—" : Number(n).toLocaleString(); }
    function holder(name) {
      var h = el("div", "dash-record-holder");
      h.appendChild(el("i", "fa-solid fa-crown"));
      h.appendChild(el("span", null, name || "—"));
      return h;
    }
    function card(opts) {
      var c = el("div", "dash-record");
      if (opts.accent) c.style.setProperty("--rec-accent", opts.accent);
      var top = el("div", "dash-record-top");
      var ico = el("span", "dash-record-ico");
      ico.appendChild(el("i", opts.icon));
      top.appendChild(ico);
      top.appendChild(el("span", "dash-record-kicker", opts.kicker));
      c.appendChild(top);
      c.appendChild(el("div", "dash-record-value", opts.value));
      if (opts.note) c.appendChild(el("span", "dash-record-note", opts.note));
      if (opts.meta) c.appendChild(el("div", "dash-record-meta", opts.meta));
      if (opts.holder) c.appendChild(holder(opts.holder));
      return c;
    }
    var recordsData = null;
    function renderRecords(d) {
      var cards = [];
      var tm = d.trove_mastery;
      if (tm) cards.push(card({
        icon: "fa-solid fa-star", accent: "#f0b429", kicker: tr("Trove Mastery"),
        value: tr("Level") + " " + num(tm.level),
        meta: num(tm.points) + " " + tr("pts") +
          (tm.points_to_next_level ? " · " + num(tm.points_to_next_level) + " " + tr("to next") : ""),
        holder: tm.player_name,
      }));
      var gm = d.geode_mastery;
      if (gm) cards.push(card({
        icon: "fa-solid fa-gem", accent: "#3fb6d4", kicker: tr("Geode Mastery"),
        value: tr("Level") + " " + num(gm.level),
        note: gm.capped ? tr("Soft cap") + " " + num(gm.level_cap) + " · " + tr("would be") + " " + num(gm.uncapped_level) : null,
        meta: num(gm.points) + " " + tr("pts"),
        holder: gm.player_name,
      }));
      var pr = d.power_rank;
      if (pr) cards.push(card({
        icon: "fa-solid fa-bolt", accent: "#c678f0", kicker: tr("Power Rank"),
        value: num(pr.value),
        meta: tr("Highest across all classes"),
        holder: pr.player_name,
      }));
      box.textContent = "";
      if (!cards.length) {
        box.appendChild(el("p", "dash-empty", tr("No records captured yet.")));
        return;
      }
      cards.forEach(function (c) { box.appendChild(c); });
    }
    getJSON("/site/leaderboards/records").then(function (d) {
      recordsData = d;
      renderRecords(d);
    }).catch(function () {
      box.textContent = "";
      box.appendChild(el("p", "dash-empty", tr("Records unavailable right now.")));
    });
    document.addEventListener("btt-lang-changed", function () {
      if (recordsData) renderRecords(recordsData);
    });
  })();

  /* ---- Official news (/site/feeds/news) ---------------------------------- */
  (function () {
    var box = document.getElementById("dash-news");
    var toggle = document.getElementById("dash-news-shop");
    if (!box) return;
    var all = [], showShop = false;   // shop offers hidden by default
    function isShop(n) { return (n.categories || []).indexOf("Shop Offers") !== -1; }
    function newsCard(n) {
      var card = el("a", "dash-card");
      card.href = n.url || "#"; card.target = "_blank"; card.rel = "noopener";
      var thumb = el("div", "dash-card-thumb");
      if (n.image) thumb.style.backgroundImage = "url('" + n.image + "')";
      if (n.category) thumb.appendChild(el("span", "dash-card-cat", n.category));
      card.appendChild(thumb);
      var body = el("div", "dash-card-body");
      body.appendChild(el("div", "dash-card-title", n.title || ""));
      var meta = el("div", "dash-card-meta");
      meta.appendChild(el("i", "fa-regular fa-clock"));
      meta.appendChild(el("span", null, timeAgo(n.published_at) || (n.author || "")));
      body.appendChild(meta);
      card.appendChild(body);
      return card;
    }
    function render() {
      box.textContent = "";
      var items = all.filter(function (n) { return showShop || !isShop(n); }).slice(0, 12);
      if (!items.length) { box.appendChild(el("p", "dash-empty", tr("No news right now."))); return; }
      items.forEach(function (n) { box.appendChild(newsCard(n)); });
    }
    if (toggle) toggle.addEventListener("click", function () {
      showShop = !showShop;
      toggle.classList.toggle("active", showShop);
      var lbl = toggle.querySelector("span"), ic = toggle.querySelector("i");
      if (lbl) lbl.textContent = showShop ? tr("Shop offers shown") : tr("Shop offers hidden");
      if (ic) ic.className = showShop ? "fa-solid fa-store" : "fa-solid fa-store-slash";
      render();
    });
    var loaded = false;
    getJSON("/site/feeds/news").then(function (d) {
      all = (d && d.items) || [];
      loaded = true;
      render();
    }).catch(function () { box.textContent = ""; box.appendChild(el("p", "dash-empty", tr("Couldn't load news."))); });
    document.addEventListener("btt-lang-changed", function () { if (loaded) render(); });
  })();

  /* ---- Community videos (/site/feeds/videos) ----------------------------- */
  (function () {
    var box = document.getElementById("dash-videos");
    var tabs = document.getElementById("dash-video-tabs");
    if (!box) return;
    var cache = {};
    var currentPlatform = "youtube";
    function render(platform) {
      currentPlatform = platform;
      box.textContent = "";
      box.appendChild(el("p", "dash-loading", tr("Loading…")));
      var done = function (items) {
        box.textContent = "";
        if (!items.length) { box.appendChild(el("p", "dash-empty", tr("No") + " " + platform + " " + tr("content right now."))); return; }
        items.slice(0, 12).forEach(function (v) {
          // YouTube items carry thumbnail_url/published_at; Twitch items carry
          // thumbnail/viewers/channel (already 440x248 from the normalizer).
          var url = v.url || "#";
          var thumbUrl = (v.thumbnail_url || v.thumbnail || "").replace("{width}", "440").replace("{height}", "248");
          var card = el("a", "dash-card");
          card.href = url; card.target = "_blank"; card.rel = "noopener";
          var thumb = el("div", "dash-card-thumb");
          if (thumbUrl) thumb.style.backgroundImage = "url('" + thumbUrl + "')";
          var badge = platform === "twitch"
            ? (typeof v.viewers === "number" ? "🔴 " + v.viewers.toLocaleString() : "")
            : timeAgo(v.published_at);
          if (badge) thumb.appendChild(el("span", "dash-card-badge", badge));
          card.appendChild(thumb);
          var body = el("div", "dash-card-body");
          body.appendChild(el("div", "dash-card-title", v.title || ""));
          var meta = el("div", "dash-card-meta");
          meta.appendChild(el("i", "fa-brands fa-" + platform));
          meta.appendChild(el("span", null, v.channel || ""));
          body.appendChild(meta);
          card.appendChild(body);
          box.appendChild(card);
        });
      };
      if (cache[platform]) { done(cache[platform]); return; }
      getJSON("/site/feeds/videos?platform=" + platform).then(function (d) {
        cache[platform] = (d && d.items) || [];
        done(cache[platform]);
      }).catch(function () { box.textContent = ""; box.appendChild(el("p", "dash-empty", tr("Couldn't load videos."))); });
    }
    if (tabs) tabs.addEventListener("click", function (e) {
      var btn = e.target.closest("button[data-vp]");
      if (!btn) return;
      tabs.querySelectorAll("button").forEach(function (b) { b.classList.toggle("active", b === btn); });
      render(btn.dataset.vp);
    });
    render("youtube");
    // Re-render the active platform on language switch. render() reads from the
    // per-platform cache (already fetched), so no duplicate network calls.
    document.addEventListener("btt-lang-changed", function () { render(currentPlatform); });
  })();

  /* ---- Players active (/site/leaderboards/activity) ---------------------- */
  (function () {
    var tile = document.getElementById("dash-activity");
    if (!tile) return;
    getJSON("/site/leaderboards/activity").then(function (a) {
      var set = function (id, v) { var e = document.getElementById(id); if (e && v != null) e.textContent = Number(v).toLocaleString(); };
      set("act-1h", a.estimate); set("act-24h", a.estimate_24h); set("act-7d", a.estimate_7d);
    }).catch(function () {});
  })();

  /* ---- Server status (/site/trove-status) -------------------------------- */
  (function () {
    var tile = document.getElementById("dash-status");
    if (!tile) return;
    var statusData = null;
    function renderStatus(d) {
      var txt = document.getElementById("dash-status-text");
      if (d.overall === "online") { tile.classList.add("is-up"); if (txt) txt.textContent = tr("Online"); }
      else if (d.overall === "down") { tile.classList.add("is-down"); if (txt) txt.textContent = tr("Down"); }
      else if (txt) txt.textContent = tr("Unknown");
    }
    getJSON("/site/trove-status").then(function (d) {
      statusData = d;
      renderStatus(d);
    }).catch(function () {});
    document.addEventListener("btt-lang-changed", function () {
      if (statusData) renderStatus(statusData);
    });
  })();

  /* ---- Latest update chip (/site/updates) -------------------------------- */
  (function () {
    var tile = document.getElementById("dash-patch");
    if (!tile) return;
    getJSON("/site/updates/live-us/versions?limit=1").then(function (d) {
      var item = d && d.items && d.items[0];
      var tag = item && (item.version_tag || item.ordinal);
      if (!tag) return;
      var txt = document.getElementById("dash-patch-text");
      if (txt) txt.textContent = tag;
      tile.hidden = false;
    }).catch(function () {});
  })();

  /* ---- Latest mods (/site/mods/projects) — right rail -------------------- */
  (function () {
    var box = document.getElementById("dash-mods");
    if (!box) return;
    var modsData = null;
    function renderMods(d) {
      var items = (d && d.items) || [];
      box.textContent = "";
      // Cap 2 mods per author so one prolific author can't monopolize the rail.
      var perAuthor = {}, picked = [];
      for (var i = 0; i < items.length && picked.length < 8; i++) {
        var m = items[i];
        if (!m.handle || !m.slug) continue;
        var key = (m.author || m.owner_username || m.handle || "").toLowerCase();
        if ((perAuthor[key] || 0) >= 2) continue;
        perAuthor[key] = (perAuthor[key] || 0) + 1;
        picked.push(m);
      }
      if (!picked.length) { box.appendChild(el("p", "dash-empty", tr("No mods yet."))); return; }
      picked.forEach(function (m) {
        var row = el("a", "dash-modrow");
        row.href = "/mods/" + encodeURIComponent(m.handle) + "/" + encodeURIComponent(m.slug);
        var thumb = el("div", "dash-modrow-thumb");
        var sha = m.banner_sha || m.preview_sha;
        if (sha) thumb.style.backgroundImage = "url('/site/mods/image/" + encodeURIComponent(sha) + "')";
        else thumb.appendChild(el("i", "fa-solid fa-cube"));
        row.appendChild(thumb);
        var body = el("div", "dash-modrow-body");
        body.appendChild(el("div", "dash-modrow-title", m.title || m.slug));
        body.appendChild(el("div", "dash-modrow-meta", m.author || m.owner_username || ""));
        row.appendChild(body);
        box.appendChild(row);
      });
    }
    getJSON("/site/mods/projects?sort=recent&limit=40").then(function (d) {
      modsData = d;
      renderMods(d);
    }).catch(function () { box.textContent = ""; box.appendChild(el("p", "dash-empty", tr("Couldn't load mods."))); });
    document.addEventListener("btt-lang-changed", function () {
      if (modsData) renderMods(modsData);
    });
  })();

  /* ---- Giveaways (/site/giveaways) --------------------------------------- */
  (function () {
    var box = document.getElementById("dash-giveaways");
    var section = box && box.closest(".dash-section");
    if (!box) return;
    var giveawaysData = null;
    function renderGiveaways(d) {
      var items = ((d && d.items) || []).filter(function (g) {
        return !g.status || g.status === "open" || g.status === "ongoing" || g.status === "active";
      });
      if (!items.length) { if (section) section.hidden = true; return; }
      box.textContent = "";
      items.slice(0, 6).forEach(function (g) {
        var card = el("div", "dash-giveaway");
        card.appendChild(el("div", "dash-giveaway-prize", g.prize_name || g.prize || g.title || tr("Giveaway")));
        var meta = el("div", "dash-giveaway-meta");
        var entries = (typeof g.entry_count === "number") ? g.entry_count : g.entries;
        if (typeof entries === "number") meta.appendChild(el("span", null, entries.toLocaleString() + " " + tr("entries")));
        var endsU = toUnix(g.ends_at);
        if (endsU) meta.appendChild(el("span", null, tr("ends in") + " " + fmtIn(endsU - Math.floor(Date.now() / 1000))));
        card.appendChild(meta);
        box.appendChild(card);
      });
    }
    getJSON("/site/giveaways").then(function (d) {
      giveawaysData = d;
      renderGiveaways(d);
    }).catch(function () { if (section) section.hidden = true; });
    document.addEventListener("btt-lang-changed", function () {
      if (giveawaysData) renderGiveaways(giveawaysData);
    });
  })();
})();

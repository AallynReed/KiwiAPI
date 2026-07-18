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
    // Every <i> we build is a decorative FontAwesome glyph - hide it from AT.
    if (tag === "i") e.setAttribute("aria-hidden", "true");
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
  var modalCard = modalEl ? modalEl.querySelector(".dash-modal-card") : null;
  var modalRelease = null;   // trapFocus release() while the modal is open
  function closeModal() {
    if (!modalEl) return;
    modalEl.hidden = true;
    document.body.style.overflow = "";
    if (modalRelease) { modalRelease(); modalRelease = null; }
  }
  function openModal(headSetup, bodyNode) {
    if (!modalEl) return;
    modalHead.textContent = ""; modalBody.textContent = "";
    modalHead.removeAttribute("style");
    headSetup(modalHead);
    if (bodyNode) modalBody.appendChild(bodyNode);
    modalEl.hidden = false;
    document.body.style.overflow = "hidden";
    // Move focus in, trap Tab, restore focus on release (Escape closes).
    if (window.BTTUtil && window.BTTUtil.trapFocus && modalCard) {
      modalRelease = window.BTTUtil.trapFocus(modalCard, { onEscape: closeModal });
    }
  }
  if (modalEl) {
    modalEl.addEventListener("click", function (e) { if (e.target.closest("[data-close]")) closeModal(); });
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
      toggle.setAttribute("aria-pressed", showShop ? "true" : "false");
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
    if (tabs) {
      var tabBtns = Array.prototype.slice.call(tabs.querySelectorAll("button[data-vp]"));
      tabs.setAttribute("role", "tablist");
      if (box) box.setAttribute("role", "tabpanel");
      var selectTab = function (btn) {
        tabBtns.forEach(function (b) {
          var on = b === btn;
          b.classList.toggle("active", on);
          b.setAttribute("aria-selected", on ? "true" : "false");
          b.tabIndex = on ? 0 : -1;
        });
        if (box) box.setAttribute("aria-labelledby", btn.id);
        render(btn.dataset.vp);
      };
      tabBtns.forEach(function (b, i) {
        if (!b.id) b.id = "dash-vp-" + b.dataset.vp;
        b.setAttribute("role", "tab");
        b.setAttribute("aria-controls", "dash-videos");
        var on = b.classList.contains("active");
        b.setAttribute("aria-selected", on ? "true" : "false");
        b.tabIndex = on ? 0 : -1;
        if (on && box) box.setAttribute("aria-labelledby", b.id);
        b.addEventListener("click", function () { selectTab(b); });
        b.addEventListener("keydown", function (e) {
          var next = null;
          if (e.key === "ArrowRight" || e.key === "ArrowDown") next = tabBtns[(i + 1) % tabBtns.length];
          else if (e.key === "ArrowLeft" || e.key === "ArrowUp") next = tabBtns[(i - 1 + tabBtns.length) % tabBtns.length];
          else if (e.key === "Home") next = tabBtns[0];
          else if (e.key === "End") next = tabBtns[tabBtns.length - 1];
          if (next) { e.preventDefault(); next.focus(); selectTab(next); }
        });
      });
    }
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

  /* ---- Yearly rotation calendar (/site/calendar/yearly) ------------------
     A ±365-day horizontal timeline: every recurring rotation (weekly buffs,
     Corruxion/Fluxion, gardening windows, Wild Mana, Stampy) as stacked bars.
     Ported from the BTT desktop app; vanilla + CSP-clean (built via the DOM,
     no innerHTML/eval), drag-to-pan, centred on today. ------------------- */
  (function () {
    var root = document.getElementById("dash-calendar");
    if (!root) return;

    var DAY_MS = 86400000;
    var TROVE_OFFSET_MS = 11 * 3600 * 1000;   // server time is UTC−11
    var DAY_W = 40, LABEL_W = 140, TOTAL_DAYS = 730;

    // Tracks (one row each). Invasion + the D15 rotation are intentionally
    // excluded to match the /v1 calendar payload.
    var TRACKS = [
      { id: "weekly_buff", name: "Weekly Buffs", color: "weekly", icon: "fa-bolt" },
      { id: "dragon_merchants", types: ["corruxion", "fluxion"], name: "Dragon Merchants", color: "corruxion", icon: "fa-dragon" },
      { id: "gardening_2", name: "2-day plants", color: "gardening", icon: "fa-seedling" },
      { id: "gardening_3", name: "3-day plants", color: "gardening", icon: "fa-seedling" },
      { id: "mana", name: "Wild Mana", color: "mana", icon: "fa-flask" },
      { id: "stampy", name: "Stampy", color: "stampy", icon: "fa-paw" }
    ];

    var state = { timeMode: "local", filter: "full" };
    var rawEvents = null;
    var wrapEl = null, todayPx = 0, barsIndex = [];
    var dragging = false, dragStartX = 0, dragScrollLeft = 0;

    function toDisplayMs(ms) { return state.timeMode === "trove" ? ms - TROVE_OFFSET_MS : ms; }

    function dayStartMs(baseMs, dayOffset) {
      if (state.timeMode !== "trove") {
        var d = new Date(baseMs);
        return new Date(d.getFullYear(), d.getMonth(), d.getDate() + dayOffset).getTime();
      }
      var s = new Date(baseMs - TROVE_OFFSET_MS);
      return Date.UTC(s.getUTCFullYear(), s.getUTCMonth(), s.getUTCDate() + dayOffset);
    }

    // toLocaleDateString/toLocaleString rebuild an Intl formatter on every call,
    // which is the calendar's per-day / per-bar cost. Cache one formatter per
    // option-set (runtime locale is fixed per session) and reuse it.
    var _dtfCache = {};
    function _dtf(opts) {
      var key = (opts.timeZone || "") + "|" + (opts.weekday || "") + "|" + (opts.month || "")
        + "|" + (opts.day || "") + "|" + (opts.hour || "") + "|" + (opts.minute || "") + "|" + (opts.year || "");
      return _dtfCache[key] || (_dtfCache[key] = new Intl.DateTimeFormat(undefined, opts));
    }
    function fmtDay(ms, options) {
      var opts = state.timeMode === "trove" ? Object.assign({ timeZone: "UTC" }, options) : options;
      return _dtf(opts).format(ms);
    }
    function fmtRange(s, e) {
      var f = _dtf({ month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
      return f.format(s) + " → " + f.format(e);
    }

    function chip(label, active, onClick, icon, toggle) {
      var b = el("button", "calendar-chip-btn" + (active ? " active" : ""));
      b.type = "button";
      // Filter/mode chips are on/off toggles -> expose their state. Action
      // chips (Today, jump-to) pass toggle falsy and stay plain buttons.
      if (toggle) b.setAttribute("aria-pressed", active ? "true" : "false");
      if (icon) { b.appendChild(el("i", "fa-solid " + icon)); b.appendChild(document.createTextNode(" ")); }
      b.appendChild(document.createTextNode(label));
      b.addEventListener("click", onClick);
      return b;
    }

    // Roving-tabindex + Left/Right/Home/End arrow nav across a chip group.
    function roveGroup(groupEl) {
      var items = Array.prototype.slice.call(groupEl.querySelectorAll("button.calendar-chip-btn"));
      if (!items.length) return;
      items.forEach(function (it, i) { it.tabIndex = i === 0 ? 0 : -1; });
      function go(i) {
        i = (i + items.length) % items.length;
        items.forEach(function (it, j) { it.tabIndex = j === i ? 0 : -1; });
        items[i].focus();
      }
      items.forEach(function (it, i) {
        it.addEventListener("keydown", function (e) {
          if (e.key === "ArrowRight" || e.key === "ArrowDown") { e.preventDefault(); go(i + 1); }
          else if (e.key === "ArrowLeft" || e.key === "ArrowUp") { e.preventDefault(); go(i - 1); }
          else if (e.key === "Home") { e.preventDefault(); go(0); }
          else if (e.key === "End") { e.preventDefault(); go(items.length - 1); }
        });
      });
    }

    function applyColor(bar, color) {
      if (!color) return;
      var hex = String(color).replace("#", "");
      if (hex.length !== 6) return;
      var r = parseInt(hex.substr(0, 2), 16), g = parseInt(hex.substr(2, 2), 16), b = parseInt(hex.substr(4, 2), 16);
      if (isNaN(r) || isNaN(g) || isNaN(b)) return;
      var dr = Math.floor(r * 0.8), dg = Math.floor(g * 0.8), db = Math.floor(b * 0.8);
      var isDark = ((dr * 299) + (dg * 587) + (db * 114)) / 1000 < 128;
      bar.style.background = "rgb(" + dr + "," + dg + "," + db + ")";
      bar.style.color = isDark ? "#fff" : "#000";
      bar.style.border = "1px solid rgba(255,255,255,0.2)";
      bar.style.textShadow = isDark ? "0 1px 2px rgba(0,0,0,0.8)" : "none";
    }

    function iconsFor(ev) {
      if (ev.biomes && ev.biomes.length) {
        var box = el("span", "calendar-ev-ics");
        ev.biomes.forEach(function (b) {
          if (!b || !b.icon) return;
          var img = document.createElement("img");
          img.src = "/static/assets/biomes/" + b.icon + ".png";
          img.alt = ""; img.loading = "lazy";
          img.onerror = function () { img.style.display = "none"; };
          box.appendChild(img);
        });
        return box.childNodes.length ? box : null;
      }
      var cls = null;
      if (ev.type === "fluxion") cls = ev.state === "selling" ? "fa-sack-dollar" : "fa-check-to-slot";
      else if (ev.type === "corruxion") cls = "fa-dragon";
      else if (ev.type.indexOf("gardening") === 0) cls = "fa-seedling";
      return cls ? el("i", "fa-solid " + cls) : null;
    }

    function labelFor(ev, tk, widthPx) {
      if (widthPx <= DAY_W) return "";
      if (tk.id === "weekly_buff") return tr(ev.name);
      if (tk.id === "dragon_merchants") {
        if (ev.type === "fluxion") return ev.state === "selling" ? tr("Selling") : tr("Voting");
        return tr("Corruxion");
      }
      return "";
    }

    function passFilter(ev, nowMs) {
      if (state.filter === "now") return ev.starts_at * 1000 <= nowMs && ev.ends_at * 1000 > nowMs;
      if (state.filter === "next") { var s = ev.starts_at * 1000; return s > nowMs && s <= nowMs + DAY_MS; }
      return true;
    }

    function makeBar(ev, tk, startTs) {
      var s = ev.starts_at * 1000, e = ev.ends_at * 1000;
      var ds = toDisplayMs(s), de = toDisplayMs(e);
      var leftPx = ((ds - startTs) / DAY_MS) * DAY_W;
      var widthPx = ((de - ds) / DAY_MS) * DAY_W;
      if (!(leftPx + widthPx > 0 && leftPx < TOTAL_DAYS * DAY_W)) return null;
      if (leftPx < 0) { widthPx += leftPx; leftPx = 0; }

      var colorClass = tk.types ? ev.type : tk.color;
      var bar = el("div", "calendar-event event-" + colorClass);
      bar.style.left = (leftPx + LABEL_W) + "px";
      bar.style.width = widthPx + "px";
      bar.style.top = "6px";
      applyColor(bar, ev.color);

      var tip = [tr(ev.name)];
      if (ev.biomes && ev.biomes.length) tip.push(ev.biomes.map(function (b) { return tr(b.name); }).join(", "));
      tip.push(fmtRange(s, e));
      bar.title = tip.join(" — ");

      var ic = iconsFor(ev);
      if (ic) bar.appendChild(ic);
      var txt = labelFor(ev, tk, widthPx);
      if (txt) bar.appendChild(document.createTextNode(txt));

      bar._startTs = s; bar._endTs = e; bar._leftPx = leftPx + LABEL_W;
      bar._type = ev.type; bar._track = tk.id;
      return bar;
    }

    function corner() { return el("div", "calendar-corner"); }

    function render(centerToday) {
      var prevScroll = wrapEl ? wrapEl.scrollLeft : null;
      root.textContent = "";
      barsIndex = [];
      if (!rawEvents) { root.appendChild(el("p", "dash-loading", tr("Loading…"))); return; }

      var nowMs = Date.now();
      var startTs = dayStartMs(nowMs, -365);
      var totalWidth = LABEL_W + TOTAL_DAYS * DAY_W;
      todayPx = ((toDisplayMs(nowMs) - startTs) / DAY_MS) * DAY_W;

      // Toolbar
      var bar = el("div", "calendar-toolbar");
      var left = el("div", "calendar-toolbar-left");
      left.appendChild(chip(tr("Today"), false, function () { centerOnToday(true); }, "fa-location-crosshairs"));
      left.appendChild(chip(tr("Full"), state.filter === "full", function () { setFilter("full"); }, null, true));
      left.appendChild(chip(tr("Now"), state.filter === "now", function () { setFilter("now"); }, null, true));
      left.appendChild(chip(tr("Next 24h"), state.filter === "next", function () { setFilter("next"); }, null, true));
      var right = el("div", "calendar-toolbar-right");
      right.appendChild(chip(tr("Local time"), state.timeMode === "local", function () { setMode("local"); }, null, true));
      right.appendChild(chip(tr("Trove time"), state.timeMode === "trove", function () { setMode("trove"); }, null, true));
      bar.appendChild(left); bar.appendChild(right);
      roveGroup(left); roveGroup(right);
      root.appendChild(bar);

      // Jump-to row
      var jump = el("div", "calendar-jump-row");
      [["corruxion", "Corruxion"], ["fluxion", "Fluxion"], ["mana", tr("Wild Mana")], ["stampy", "Stampy"]]
        .forEach(function (j) { jump.appendChild(chip(j[1], false, function () { jumpTo(j[0]); })); });
      jump.appendChild(el("span", "calendar-helper-text", tr("Drag to pan • scroll to move")));
      roveGroup(jump);
      root.appendChild(jump);

      // Timeline wrapper
      var wrap = el("div", "calendar-timeline-wrapper draggable");

      var line = el("div", "calendar-today-line");
      line.style.left = (todayPx + LABEL_W) + "px";
      wrap.appendChild(line);

      // Month + day headers.
      // Weekday short-names depend only on the (local) day-of-week, so there are
      // exactly 7 distinct values. Formatting all 730 days through Intl
      // (toLocaleDateString) was the dominant render cost; precompute the 7
      // labels once and index by getDay() for byte-identical output.
      // Index by the same timezone fmtDay formats in (UTC in Trove-time mode),
      // so the label matches the day it's placed on.
      var troveTz = state.timeMode === "trove";
      var weekdayLabels = [];
      for (var w = 0; w < 7; w++) {
        var wMs = startTs + w * DAY_MS, wDate = new Date(wMs);
        weekdayLabels[troveTz ? wDate.getUTCDay() : wDate.getDay()] = fmtDay(wMs, { weekday: "short" });
      }
      var months = [], days = [], curKey = null, cur = null;
      for (var i = 0; i < TOTAL_DAYS; i++) {
        var ms = startTs + i * DAY_MS, dd = new Date(ms);
        var key = state.timeMode === "trove"
          ? dd.getUTCFullYear() + "-" + dd.getUTCMonth()
          : dd.getFullYear() + "-" + dd.getMonth();
        if (key !== curKey) {
          if (cur) months.push(cur);
          curKey = key;
          cur = { name: fmtDay(ms, { month: "long" }), year: state.timeMode === "trove" ? dd.getUTCFullYear() : dd.getFullYear(), days: 0 };
        }
        cur.days++;
        days.push({ isToday: i === 365, num: state.timeMode === "trove" ? dd.getUTCDate() : dd.getDate(), weekday: weekdayLabels[troveTz ? dd.getUTCDay() : dd.getDay()] });
      }
      if (cur) months.push(cur);

      var header = el("div", "calendar-timeline-header");
      header.style.width = totalWidth + "px";
      var mrow = el("div", "calendar-months-row");
      mrow.appendChild(corner());
      months.forEach(function (m) {
        var col = el("div", "calendar-month-col");
        col.style.width = (m.days * DAY_W) + "px";
        col.appendChild(el("div", "calendar-month-label", m.name + " " + m.year));
        mrow.appendChild(col);
      });
      var drow = el("div", "calendar-days-row");
      drow.appendChild(corner());
      days.forEach(function (d) {
        var col = el("div", "calendar-day-col" + (d.isToday ? " is-today" : ""));
        col.appendChild(el("div", "calendar-day-weekday", d.weekday));
        col.appendChild(el("div", "calendar-day-num", String(d.num)));
        drow.appendChild(col);
      });
      header.appendChild(mrow); header.appendChild(drow);
      wrap.appendChild(header);

      // Tracks
      var tracksEl = el("div", "calendar-tracks");
      tracksEl.style.width = totalWidth + "px";
      var anyTrack = false;
      TRACKS.forEach(function (tk) {
        var evs = rawEvents.filter(function (e) {
          if (!passFilter(e, nowMs)) return false;
          return tk.types ? tk.types.indexOf(e.type) >= 0 : e.type === tk.id;
        });
        var bars = [];
        evs.forEach(function (ev) { var b = makeBar(ev, tk, startTs); if (b) bars.push(b); });
        if (!bars.length) return;
        anyTrack = true;
        var row = el("div", "calendar-track");
        var lbl = el("div", "calendar-track-label");
        lbl.appendChild(el("i", "fa-solid " + tk.icon));
        lbl.appendChild(document.createTextNode(" " + tr(tk.name)));
        row.appendChild(lbl);
        bars.forEach(function (b) { row.appendChild(b); barsIndex.push(b); });
        tracksEl.appendChild(row);
      });
      if (!anyTrack) {
        var empty = el("div", "calendar-empty-state");
        empty.appendChild(el("i", "fa-regular fa-calendar-xmark"));
        empty.appendChild(document.createTextNode(" " + tr("No entries match this filter right now.")));
        tracksEl.appendChild(empty);
      }
      wrap.appendChild(tracksEl);

      root.appendChild(wrap);
      wrapEl = wrap;
      attachDrag(wrap);

      if (centerToday) centerOnToday(false);
      else if (prevScroll != null) wrap.scrollLeft = prevScroll;
    }

    function centerOnToday(animate) {
      if (!wrapEl) return;
      wrapEl.style.scrollBehavior = animate ? "smooth" : "auto";
      wrapEl.scrollLeft = todayPx + LABEL_W - (wrapEl.clientWidth / 2);
      if (animate) setTimeout(function () { if (wrapEl) wrapEl.style.scrollBehavior = "auto"; }, 500);
    }

    function jumpTo(target) {
      if (!wrapEl || !barsIndex.length) return;
      var nowMs = Date.now();
      var cands = barsIndex.filter(function (b) {
        if (target === "mana") return b._track === "mana";
        if (target === "stampy") return b._track === "stampy";
        return b._type === target;
      });
      if (!cands.length) return;
      cands.sort(function (a, b) {
        var ad = a._startTs >= nowMs ? a._startTs - nowMs : Math.abs(nowMs - a._startTs) + 9e11;
        var bd = b._startTs >= nowMs ? b._startTs - nowMs : Math.abs(nowMs - b._startTs) + 9e11;
        return ad - bd;
      });
      wrapEl.style.scrollBehavior = "smooth";
      wrapEl.scrollLeft = Math.max(0, cands[0]._leftPx - wrapEl.clientWidth * 0.35);
      setTimeout(function () { if (wrapEl) wrapEl.style.scrollBehavior = "auto"; }, 400);
    }

    function setFilter(f) { if (state.filter === f) return; state.filter = f; render(false); }
    function setMode(m) { if (state.timeMode === m) return; state.timeMode = m; render(true); }

    function attachDrag(w) {
      w.addEventListener("mousedown", function (e) {
        dragging = true; dragStartX = e.pageX - w.offsetLeft; dragScrollLeft = w.scrollLeft; w.classList.add("dragging");
      });
      w.addEventListener("mousemove", function (e) {
        if (!dragging) return; e.preventDefault();
        w.scrollLeft = dragScrollLeft - ((e.pageX - w.offsetLeft) - dragStartX) * 1.5;
      });
      var stop = function () { dragging = false; w.classList.remove("dragging"); };
      w.addEventListener("mouseup", stop);
      w.addEventListener("mouseleave", stop);
      w.addEventListener("wheel", function (e) {
        if (e.deltaY !== 0) { e.preventDefault(); w.scrollLeft += e.deltaY; }
      }, { passive: false });
    }

    getJSON("/site/calendar/yearly").then(function (d) {
      rawEvents = (d && d.events) || [];
      render(true);
    }).catch(function () {
      root.textContent = "";
      root.appendChild(el("p", "dash-empty", tr("Couldn't load the calendar.")));
    });
    document.addEventListener("btt-lang-changed", function () { if (rawEvents) render(false); });
  })();
})();

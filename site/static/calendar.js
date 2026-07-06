/* ===========================================================================
   calendar.js - the live Trove calendar (/calendar).

   One board for every rotation and event, each with a live countdown:
     resets + buffs + chaos + merchants/biomes  -> /site/rotations (shared)
     ongoing / upcoming events                  -> /site/calendar/events

   All of it fails closed: a section that errors just shows an empty state.
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
  function pad(n) { return n < 10 ? "0" + n : "" + n; }
  function fmtIn(sec) {
    if (sec == null) return "—";
    if (sec <= 0) return tr("now");
    var d = Math.floor(sec / 86400), h = Math.floor((sec % 86400) / 3600), m = Math.floor((sec % 3600) / 60);
    if (d > 0) return d + "d " + h + "h";
    if (h > 0) return h + "h " + m + "m";
    if (m > 0) return m + "m";
    return sec + "s";
  }
  function fmtDate(unix) {
    if (!unix) return "";
    var d = new Date(unix * 1000);
    return d.toLocaleDateString(undefined, { month: "short", day: "numeric" }) + " " +
      d.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
  }
  function biomePill(b) {
    var name = (typeof b === "string") ? b : ((b && b.name) || "");
    var icon = (b && typeof b === "object") ? b.icon : null;
    var span = el("span", "cal-biome-pill");
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

  /* ---- Shared modal ------------------------------------------------------ */
  var modalEl = document.getElementById("cal-modal");
  var modalHead = document.getElementById("cal-modal-head");
  var modalBody = document.getElementById("cal-modal-body");
  function closeModal() { if (modalEl) { modalEl.hidden = true; document.body.style.overflow = ""; } }
  function openModal(title, bodyNode) {
    if (!modalEl) return;
    modalHead.textContent = title;
    modalBody.textContent = "";
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

  // Live countdown anchor: everything ticks off one server-synced clock so the
  // page stays correct even if the visitor's local clock is skewed.
  var anchor = Math.floor(Date.now() / 1000), t0 = Date.now();
  function nowU() { return Math.floor(anchor + (Date.now() - t0) / 1000); }
  var tickers = [];   // fns re-run every second to refresh live countdowns
  (function tick() { var n = nowU(); tickers.forEach(function (f) { f(n); }); setTimeout(tick, 1000); })();

  // Stored payloads so we can re-render JS-built labels on a live language
  // switch (btt-lang-changed). Both rotations + events builders push tickers,
  // so a re-render resets the tickers array first to avoid stacking duplicates.
  var rotationsData = null, eventsData = null;

  /* ---- Rotations (/site/rotations) --------------------------------------- */
  function renderRotations(d) {
    var st = d.server_time || {};
    if (typeof st.now_unix === "number") { anchor = st.now_unix; t0 = Date.now(); }

    // Reset strip + server clock
    var dEl = document.getElementById("cal-daily"), wEl = document.getElementById("cal-weekly"),
        cEl = document.getElementById("cal-clock");
    tickers.push(function (n) {
      if (dEl && st.daily_reset_at) dEl.textContent = tr("in") + " " + fmtIn(st.daily_reset_at - n);
      if (wEl && st.weekly_reset_at) wEl.textContent = tr("in") + " " + fmtIn(st.weekly_reset_at - n);
      if (cEl) {
        var t = new Date((n - 11 * 3600) * 1000);
        cEl.textContent = pad(t.getUTCHours()) + ":" + pad(t.getUTCMinutes()) + ":" + pad(t.getUTCSeconds());
      }
    });

    // Merchants + biomes
    var mEl = document.getElementById("cal-merchants");
    if (mEl) {
      mEl.textContent = "";
      var merchants = d.merchants || [];
      if (!merchants.length) mEl.appendChild(el("p", "cal-empty", tr("No rotations available right now.")));
      merchants.forEach(function (m) { mEl.appendChild(merchantCard(m)); });
    }

    // Bonuses + chaos
    var bEl = document.getElementById("cal-buffs");
    if (bEl) {
      bEl.textContent = "";
      var db = buffCard(tr("Today's bonus"), d.daily_buff, d.daily_rotation, tr("Daily bonus rotation"));
      var wb = buffCard(tr("This week's bonus"), d.weekly_buff, d.weekly_rotation, tr("Weekly bonus rotation"));
      if (db) bEl.appendChild(db);
      if (wb) bEl.appendChild(wb);
      if (d.chaos && d.chaos.ends_at) bEl.appendChild(chaosCard(d.chaos));
      if (!bEl.children.length) bEl.appendChild(el("p", "cal-empty", tr("No bonuses available right now.")));
    }

    function merchantCard(m) {
      var card = el("div", "cal-merchant " + (m.active ? "is-active" : "is-inactive"));
      var top = el("div", "cal-merchant-top");
      top.appendChild(el("span", "cal-merchant-name", m.name));
      top.appendChild(el("span", "cal-merchant-badge", m.active ? (m.state || tr("Here")) : tr("Away")));
      card.appendChild(top);
      var time = el("div", "cal-merchant-time");
      card.appendChild(time);
      tickers.push(function (n) {
        var target = m.active ? m.ends_at : m.starts_at;
        if (target) time.textContent = (m.active ? tr("Leaves in") : tr("Returns in")) + " " + fmtIn(target - n);
      });
      if (m.biomes && m.biomes.length) {
        var bs = el("div", "cal-merchant-biomes");
        m.biomes.slice(0, 4).forEach(function (b) { bs.appendChild(biomePill(b)); });
        card.appendChild(bs);
      }
      return clickable(card, function () { merchantModal(m); });
    }

    function buffCard(kicker, buff, rotation, title) {
      if (!buff) return null;
      var card = el("div", "cal-buff");
      if (buff.color) card.style.setProperty("--buff-accent", "#" + buff.color);
      card.appendChild(el("span", "cal-buff-kicker", kicker));
      card.appendChild(el("div", "cal-buff-name", (buff.emoji ? buff.emoji + " " : "") + (buff.name || "")));
      var list = el("ul", "cal-buff-list");
      (buff.normal_buffs || buff.buffs || []).slice(0, 4).forEach(function (b) { list.appendChild(el("li", null, b)); });
      card.appendChild(list);
      return clickable(card, function () { rotationModal(title, rotation || []); });
    }

    function chaosCard(chaos) {
      var card = el("div", "cal-buff cal-buff-chaos");
      card.appendChild(el("span", "cal-buff-kicker", tr("Chaos Chest")));
      var item = chaos.item;
      card.appendChild(el("div", "cal-buff-name", (item && item.name) ? item.name : tr("Featured item")));
      var when = el("div", "cal-buff-when");
      card.appendChild(when);
      tickers.push(function (n) { if (chaos.ends_at) when.textContent = tr("Resets in") + " " + fmtIn(chaos.ends_at - n); });
      return clickable(card, function () { chaosModal(chaos); });
    }
  }
  getJSON("/site/rotations").then(function (d) {
    rotationsData = d;
    renderRotations(d);
  }).catch(function () {
    setEmpty("cal-merchants", tr("Couldn't load rotations."));
    setEmpty("cal-buffs", tr("Couldn't load bonuses."));
  });

  /* ---- Events (/site/calendar/events) ------------------------------------ */
  function renderEvents(d) {
    var box = document.getElementById("cal-events");
    if (!box) return;
    box.textContent = "";
    var ongoing = d.ongoing || [], upcoming = d.upcoming || [];
    if (!ongoing.length && !upcoming.length) {
      box.appendChild(el("p", "cal-empty", tr("No events scheduled right now.")));
      return;
    }
    if (ongoing.length) {
      box.appendChild(el("p", "cal-events-group-title", tr("Happening now")));
      ongoing.forEach(function (ev) { box.appendChild(eventCard(ev, true)); });
    }
    if (upcoming.length) {
      box.appendChild(el("p", "cal-events-group-title", tr("Coming up")));
      upcoming.forEach(function (ev) { box.appendChild(eventCard(ev, false)); });
    }
  }
  getJSON("/site/calendar/events").then(function (d) {
    eventsData = d;
    renderEvents(d);
  }).catch(function () { setEmpty("cal-events", tr("Couldn't load events.")); });

  function eventCard(ev, ongoing) {
    var card = el("a", "cal-event " + (ongoing ? "is-ongoing" : "is-upcoming"));
    card.href = ev.url || "#";
    if (ev.url) { card.target = "_blank"; card.rel = "noopener"; }
    if (ev.icon) {
      var img = document.createElement("img");
      img.src = ev.icon; img.alt = ""; img.className = "cal-event-ic"; img.loading = "lazy";
      img.onerror = function () { var f = el("span", "cal-event-ic"); f.appendChild(el("i", "fa-solid fa-calendar-day")); img.replaceWith(f); };
      card.appendChild(img);
    } else {
      var ic = el("span", "cal-event-ic"); ic.appendChild(el("i", "fa-solid fa-calendar-day")); card.appendChild(ic);
    }
    var body = el("div", "cal-event-body");
    body.appendChild(el("div", "cal-event-name", ev.name || ""));
    var meta = el("div", "cal-event-meta");
    if (ev.category) meta.appendChild(el("span", "cal-event-cat", ev.category));
    var when = el("span", "cal-event-when");
    meta.appendChild(when);
    tickers.push(function (n) {
      when.textContent = ongoing ? tr("ends in") + " " + fmtIn(ev.ends_at - n) : tr("starts in") + " " + fmtIn(ev.starts_at - n);
    });
    body.appendChild(meta);
    card.appendChild(body);
    return card;
  }

  /* ---- Modal renderers --------------------------------------------------- */
  function rotationModal(title, entries) {
    var body = el("div"), n = nowU();
    if (!entries.length) {
      body.appendChild(el("p", "cal-modal-note", tr("Rotation unavailable right now.")));
    } else {
      var ul = el("ul", "cal-rot");
      entries.forEach(function (e) {
        var li = el("li", "cal-rot-row" + (e.is_current ? " is-now" : ""));
        if (e.color) li.style.setProperty("--buff-accent", "#" + e.color);
        var top = el("div", "cal-rot-top");
        var name = el("div", "cal-rot-name");
        if (e.emoji) name.appendChild(el("span", null, e.emoji));
        name.appendChild(el("span", null, e.name || ""));
        top.appendChild(name);
        top.appendChild(el("span", "cal-rot-when" + (e.is_current ? " is-now" : ""),
          e.is_current ? tr("Active now") : tr("in") + " " + fmtIn((e.next_at || 0) - n)));
        li.appendChild(top);
        var bl = e.normal_buffs || e.buffs || [];
        if (bl.length) li.appendChild(el("div", "cal-rot-buffs", bl.join(" · ")));
        ul.appendChild(li);
      });
      body.appendChild(ul);
    }
    openModal(title, body);
  }

  function chaosModal(chaos) {
    var body = el("div"), n = nowU(), item = chaos.item;
    if (item && item.name) {
      body.appendChild(el("p", "cal-modal-note", tr("Featured item this week")));
      body.appendChild(el("p", "cal-buff-name", item.name));
    } else {
      body.appendChild(el("p", "cal-modal-note", tr("The featured item rotates every week (not captured yet).")));
    }
    body.appendChild(el("p", null, tr("Window:") + " " + fmtDate(chaos.starts_at) + " → " + fmtDate(chaos.ends_at)));
    if (chaos.ends_at) body.appendChild(el("p", null, tr("Resets in") + " " + fmtIn(chaos.ends_at - n)));
    openModal("🎁 " + tr("Chaos Chest"), body);
  }

  function merchantModal(m) {
    var body = el("div"), n = nowU();
    var status = m.active
      ? tr("Here now") + (m.ends_at ? " · " + tr("leaves in") + " " + fmtIn(m.ends_at - n) : "")
      : (m.starts_at ? tr("Returns in") + " " + fmtIn(m.starts_at - n) : tr("Away"));
    body.appendChild(el("p", "cal-modal-note", status));
    var sched = m.schedule || [];
    if (!sched.length) {
      body.appendChild(el("p", "cal-modal-note", tr("No upcoming schedule available.")));
    } else {
      var ul = el("ul", "cal-sched");
      sched.forEach(function (s) {
        var isNow = s.starts_at <= n && s.ends_at > n;
        var li = el("li", "cal-sched-row" + (isNow ? " is-now" : ""));
        var time = el("div", "cal-sched-time");
        time.appendChild(el("span", null, fmtDate(s.starts_at) + " – " + fmtDate(s.ends_at)));
        if (isNow) time.appendChild(el("span", "cal-sched-now", tr("Now")));
        else if (s.state) time.appendChild(el("span", "cal-sched-state", s.state));
        li.appendChild(time);
        if (s.biomes && s.biomes.length) {
          var bs = el("div", "cal-sched-biomes");
          s.biomes.forEach(function (x) { bs.appendChild(biomePill(x)); });
          li.appendChild(bs);
        }
        ul.appendChild(li);
      });
      body.appendChild(ul);
    }
    openModal(m.name, body);
  }

  function setEmpty(id, msg) {
    var box = document.getElementById(id);
    if (box) { box.textContent = ""; box.appendChild(el("p", "cal-empty", msg)); }
  }

  // Re-render JS-built labels when the locale dict loads / language switches.
  // Both builders push tickers, so we drop the whole tickers array first and
  // let the re-render repopulate it — otherwise countdowns would stack and run
  // multiple times per second. The persistent tick() loop reads `tickers` by
  // reference each second, so reassigning it here is safe.
  document.addEventListener("btt-lang-changed", function () {
    if (!rotationsData && !eventsData) return;
    tickers = [];
    if (rotationsData) renderRotations(rotationsData);
    if (eventsData) renderEvents(eventsData);
  });
})();

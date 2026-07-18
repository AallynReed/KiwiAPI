/* ===========================================================================
   streams.js - the Trove community hub (/streams).

     live Twitch streams  -> /site/feeds/videos?platform=twitch
     recent YouTube videos -> /site/feeds/videos?platform=youtube
     latest official news  -> /site/feeds/news

   All shared, same-origin, cached proxies. Every section fails closed.
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
  function timeAgo(ts) {
    if (!ts) return "";
    var t = (typeof ts === "number") ? ts * 1000 : Date.parse(ts);
    if (!t) return "";
    var s = Math.max(0, (Date.now() - t) / 1000);
    if (s < 3600) return Math.floor(s / 60) + tr("m ago");
    if (s < 86400) return Math.floor(s / 3600) + tr("h ago");
    return Math.floor(s / 86400) + tr("d ago");
  }
  // Decorative icon: always aria-hidden so screen readers skip it.
  function icon(cls) { var i = el("i", cls); i.setAttribute("aria-hidden", "true"); return i; }
  function fallbackThumb(t) {
    var f = el("div", "stm-thumb-fallback"); f.appendChild(icon("fa-solid fa-image")); t.appendChild(f);
  }
  // Thumbnails render as a lazy, async-decoded <img> (not a background-image) so
  // the ~40 stream/video/news images load only as they scroll into view.
  function thumb(url) {
    var t = el("div", "stm-thumb");
    if (url) {
      var img = document.createElement("img");
      img.className = "stm-thumb-img";
      img.src = String(url);
      img.alt = ""; img.loading = "lazy"; img.decoding = "async";
      img.onerror = function () { img.remove(); fallbackThumb(t); };
      t.appendChild(img);
    } else {
      fallbackThumb(t);
    }
    return t;
  }
  function setEmpty(id, msg) {
    var box = document.getElementById(id);
    if (box) { box.textContent = ""; box.appendChild(el("p", "stm-empty", msg)); }
  }

  // Stored payloads so we can re-render on a live language change.
  var twitchData = null, youtubeData = null, newsData = null;

  /* ---- Live Twitch streams ----------------------------------------------- */
  function renderTwitch(d) {
    var box = document.getElementById("stm-streams");
    if (!box) return;
    var items = (d && d.items) || [];
    box.textContent = "";
    var cnt = document.getElementById("stm-tw-count");
    if (cnt) cnt.textContent = items.length ? items.length + " " + (items.length === 1 ? tr("streamer live") : tr("streamers live")) : "";
    if (!items.length) { box.appendChild(el("p", "stm-empty", tr("Nobody's streaming Trove right now - check back later."))); return; }
    items.slice(0, 16).forEach(function (s) {
      var card = el("a", "stm-card");
      card.href = s.url || "#"; card.target = "_blank"; card.rel = "noopener";
      var th = thumb((s.thumbnail || "").replace("{width}", "440").replace("{height}", "248"));
      var live = el("span", "stm-live"); live.appendChild(el("span", "stm-live-dot")); live.appendChild(el("span", null, tr("Live")));
      th.appendChild(live);
      if (typeof s.viewers === "number") th.appendChild(el("span", "stm-viewers", "👁 " + s.viewers.toLocaleString()));
      card.appendChild(th);
      var body = el("div", "stm-card-body");
      body.appendChild(el("div", "stm-card-title", s.title || ""));
      var meta = el("div", "stm-card-meta");
      meta.appendChild(icon("fa-brands fa-twitch"));
      meta.appendChild(el("span", null, s.channel || s.login || ""));
      body.appendChild(meta);
      card.appendChild(body);
      box.appendChild(card);
    });
  }
  (function () {
    if (!document.getElementById("stm-streams")) return;
    getJSON("/site/feeds/videos?platform=twitch").then(function (d) {
      twitchData = d || { items: [] };
      renderTwitch(twitchData);
    }).catch(function () { setEmpty("stm-streams", tr("Couldn't load streams.")); });
  })();

  /* ---- Recent YouTube videos --------------------------------------------- */
  function renderYouTube(d) {
    var box = document.getElementById("stm-videos");
    if (!box) return;
    var items = (d && d.items) || [];
    box.textContent = "";
    if (!items.length) { box.appendChild(el("p", "stm-empty", tr("No recent videos right now."))); return; }
    items.slice(0, 12).forEach(function (v) {
      var card = el("a", "stm-card");
      card.href = v.url || "#"; card.target = "_blank"; card.rel = "noopener";
      var th = thumb(v.thumbnail_url || v.thumbnail);
      var ago = timeAgo(v.published_at);
      if (ago) th.appendChild(el("span", "stm-badge", ago));
      card.appendChild(th);
      var body = el("div", "stm-card-body");
      body.appendChild(el("div", "stm-card-title", v.title || ""));
      var meta = el("div", "stm-card-meta");
      meta.appendChild(icon("fa-brands fa-youtube"));
      meta.appendChild(el("span", null, v.channel || ""));
      body.appendChild(meta);
      card.appendChild(body);
      box.appendChild(card);
    });
  }
  (function () {
    if (!document.getElementById("stm-videos")) return;
    getJSON("/site/feeds/videos?platform=youtube").then(function (d) {
      youtubeData = d || { items: [] };
      renderYouTube(youtubeData);
    }).catch(function () { setEmpty("stm-videos", tr("Couldn't load videos.")); });
  })();

  /* ---- Latest official news ---------------------------------------------- */
  function renderNews(d) {
    var box = document.getElementById("stm-news");
    if (!box) return;
    // Hide shop-offer spam; keep real news.
    var items = ((d && d.items) || []).filter(function (n) {
      return (n.categories || []).indexOf("Shop Offers") === -1;
    });
    box.textContent = "";
    if (!items.length) { box.appendChild(el("p", "stm-empty", tr("No news right now."))); return; }
    items.slice(0, 12).forEach(function (n) {
      var card = el("a", "stm-card");
      card.href = n.url || "#"; card.target = "_blank"; card.rel = "noopener";
      var th = thumb(n.image);
      if (n.category) th.appendChild(el("span", "stm-news-cat", n.category));
      card.appendChild(th);
      var body = el("div", "stm-card-body");
      body.appendChild(el("div", "stm-card-title", n.title || ""));
      var meta = el("div", "stm-card-meta");
      meta.appendChild(icon("fa-regular fa-clock"));
      meta.appendChild(el("span", null, timeAgo(n.published_at) || (n.author || "")));
      body.appendChild(meta);
      card.appendChild(body);
      box.appendChild(card);
    });
  }
  (function () {
    if (!document.getElementById("stm-news")) return;
    getJSON("/site/feeds/news").then(function (d) {
      newsData = d || { items: [] };
      renderNews(newsData);
    }).catch(function () { setEmpty("stm-news", tr("Couldn't load news.")); });
  })();

  /* ---- Re-render labels on a live language change ------------------------- */
  document.addEventListener("btt-lang-changed", function () {
    if (twitchData) renderTwitch(twitchData);
    if (youtubeData) renderYouTube(youtubeData);
    if (newsData) renderNews(newsData);
  });
})();

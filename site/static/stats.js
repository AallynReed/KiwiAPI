// Anonymous page-view counter: one report when a page opens, how fast it loaded, then
// how long it stayed in the foreground. No cookies and nothing stored; the server keeps
// only coarse labels (see app/pageviews/collect.py). ?nostats=1 stops counting this
// browser, ?nostats=0 resumes.
(function () {
  'use strict';
  try {
    var q = new URLSearchParams(location.search);
    if (q.has('nostats')) {
      if (q.get('nostats') === '0') localStorage.removeItem('btt_nostats');
      else localStorage.setItem('btt_nostats', '1');
    }
    if (localStorage.getItem('btt_nostats') === '1') return;
  } catch (e) { /* storage blocked: count anyway */ }
  if (navigator.webdriver) return;

  var id, visibleSince, total, lastSent;

  function newId() {
    if (window.crypto && crypto.randomUUID) return crypto.randomUUID();
    return Date.now().toString(36) + '-' + Math.random().toString(36).slice(2, 12);
  }

  function send(data) {
    data.id = data.id || id;
    var body = JSON.stringify(data);
    try {
      if (navigator.sendBeacon && navigator.sendBeacon('/s', body)) return;
    } catch (e) { /* fall through to fetch */ }
    try {
      fetch('/s', { method: 'POST', body: body, keepalive: true, credentials: 'omit',
        headers: { 'Content-Type': 'text/plain' } }).catch(function () {});
    } catch (e) { /* best effort */ }
  }

  function seconds() {
    return Math.round((total + (visibleSince ? Date.now() - visibleSince : 0)) / 1000);
  }

  function reportTime() {
    var s = seconds();
    if (s > lastSent) { lastSent = s; send({ t: 'end', s: s }); }
  }

  function lang() {
    try { if (window.BTTi18n) return window.BTTi18n.current(); } catch (e) { /* ignore */ }
    return document.documentElement.lang || 'en';
  }

  function start() {
    id = newId();
    visibleSince = document.visibilityState === 'visible' ? Date.now() : 0;
    total = 0;
    lastSent = 0;
    send({ t: 'pv', u: location.href, r: document.referrer, l: lang(),
      w: screen.width || 0, tp: navigator.maxTouchPoints || 0 });
    measure();
  }

  // How fast this page loaded on this device, sent once: shortly after `load` (the
  // largest paint settles), or when the visitor leaves first. Times count from when the
  // page was shown, so a prerendered page isn't charged for loading in the background.
  // A page restored from the back/forward cache didn't load, so it sends nothing.
  var measured = false;
  function measure() {
    var P = window.performance;
    if (measured || !P || !P.getEntriesByType || !window.PerformanceObserver) return;
    measured = true;
    var perfId = id, fcp = 0, lcp = 0, sent = false;
    function observe(type, fn) {
      try {
        new PerformanceObserver(function (l) { l.getEntries().forEach(fn); }).observe({ type: type, buffered: true });
        return true;
      } catch (e) { return false; }
    }
    observe('paint', function (e) { if (e.name === 'first-contentful-paint') fcp = e.startTime; });
    observe('largest-contentful-paint', function (e) { lcp = e.startTime; });
    // Layout shift (CLS) the way Google scores it: the worst burst of shifts (gaps under
    // 1 s, at most 5 s), shifts right after input excluded. It grows while the page is
    // open, so it goes again each time the page is hidden.
    var cls = 0, burst = 0, first = 0, prev = 0, clsSent = -1;
    var clsOn = observe('layout-shift', function (e) {
      if (e.hadRecentInput) return;
      if (burst && e.startTime - prev < 1000 && e.startTime - first < 5000) burst += e.value;
      else { burst = e.value; first = e.startTime; }
      prev = e.startTime;
      if (burst > cls) cls = burst;
    });
    function report() {
      if (sent) return;
      sent = true;
      var n = P.getEntriesByType('navigation')[0] || {}, shown = n.activationStart || 0;
      function ms(v) { return v ? Math.max(1, Math.round(v - shown)) : 0; }
      send({ t: 'perf', id: perfId, ttfb: ms(n.responseStart), fcp: ms(fcp), lcp: ms(lcp),
        dcl: ms(n.domContentLoadedEventEnd), load: ms(n.loadEventEnd) });
    }
    function leaving() {
      report();
      var c = Math.round(cls * 1000);
      if (clsOn && c > clsSent) { clsSent = c; send({ t: 'cls', id: perfId, c: c }); }
    }
    function later() { setTimeout(report, 1500); }
    if (document.readyState === 'complete') later();
    else addEventListener('load', later);
    document.addEventListener('visibilitychange', function () {
      if (document.visibilityState === 'hidden') leaving();
    });
    addEventListener('pagehide', leaving);
  }

  document.addEventListener('visibilitychange', function () {
    if (!id) return;
    if (document.visibilityState === 'hidden') {
      if (visibleSince) { total += Date.now() - visibleSince; visibleSince = 0; }
      reportTime();
    } else if (!visibleSince) {
      visibleSince = Date.now();
    }
  });
  addEventListener('pagehide', function () { if (id) reportTime(); });
  // Restored from the back/forward cache: a fresh view of the page.
  addEventListener('pageshow', function (e) { if (e.persisted) start(); });

  // In-app browsers often close without pagehide, so report as we go.
  var beats = [4, 4, 7, 15, 30], n = 0;
  (function beat() {
    setTimeout(function () { if (id && visibleSince) reportTime(); beat(); }, (beats[n++] || 60) * 1000);
  })();

  // After DOMContentLoaded, so i18n.js has picked the page language. `load` covers a
  // script that arrives after DOMContentLoaded already fired.
  var begun = false;
  function begin() {
    if (begun) return;
    begun = true;
    if (document.prerendering) document.addEventListener('prerenderingchange', start, { once: true });
    else start();
  }
  if (document.readyState === 'complete') begin();
  else {
    document.addEventListener('DOMContentLoaded', begin);
    addEventListener('load', begin);
  }
})();

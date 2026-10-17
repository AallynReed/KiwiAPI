// Anonymous page-view counter: one report when a page opens, then how long it stayed
// in the foreground. No cookies and nothing stored; the server keeps only coarse
// labels (see app/pageviews/collect.py). ?nostats=1 stops counting this browser,
// ?nostats=0 resumes.
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
    data.id = id;
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

/* ===========================================================================
   design_feedback.js - "Do you like the new design?" on the Voxel homepage.

   Shown once per browser, after ~25 s of the page actually being on screen, in
   the bottom-left corner (the support pill owns the right). A vote posts an
   anonymous tally to /site/design-feedback (design + theme + up/down, nothing
   about the visitor); the answer or a dismissal is remembered in localStorage
   so it never asks again. It never takes focus.
   ========================================================================== */
(function () {
  "use strict";

  var KEY = "btt-design-feedback";
  var DELAY_MS = 25000;
  var box = document.getElementById("vx-feedback");
  if (!box) return;
  function tr(s) { return window.BTTi18n && window.BTTi18n.t ? window.BTTi18n.t(s) : s; }
  function remembered() { try { return localStorage.getItem(KEY); } catch (_) { return "unavailable"; } }
  function remember(v) { try { localStorage.setItem(KEY, v); } catch (_) {} }
  if (remembered()) return;

  var reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  var shown = 0, since = Date.now(), timer = 0;

  function show() {
    box.hidden = false;
    if (reduce) return;
    box.classList.add("is-entering");
    requestAnimationFrame(function () { requestAnimationFrame(function () { box.classList.remove("is-entering"); }); });
  }
  function hide() {
    if (reduce) { box.hidden = true; return; }
    box.classList.add("is-entering");
    setTimeout(function () { box.hidden = true; }, 350);
  }
  function schedule() {
    clearTimeout(timer);
    if (document.visibilityState !== "visible") return;
    since = Date.now();
    timer = setTimeout(show, Math.max(0, DELAY_MS - shown));
  }
  document.addEventListener("visibilitychange", function () {
    if (document.visibilityState === "visible") schedule();
    else { shown += Date.now() - since; clearTimeout(timer); }
  });
  schedule();

  function vote(v) {
    remember(v);
    var theme = window.BTTAppearance ? window.BTTAppearance.theme() : "auto";
    fetch("/site/design-feedback", {
      method: "POST",
      headers: { "Content-Type": "application/json", Accept: "application/json" },
      body: JSON.stringify({ design: "voxel", theme: theme, vote: v }),
    }).catch(function () {});
    var thanks = document.getElementById("vx-feedback-thanks");
    box.classList.add("is-done");
    if (v === "up") {
      thanks.textContent = tr("Thanks! Glad you like it.");
      setTimeout(hide, 5000);
    } else {
      thanks.textContent = tr("Thanks for telling us.") + " ";
      var sw = document.createElement("button");
      sw.type = "button";
      sw.textContent = tr("Switch to the classic look");
      sw.addEventListener("click", function () {
        if (window.BTTAppearance) window.BTTAppearance.setDesign("classic");
      });
      thanks.appendChild(sw);
      setTimeout(hide, 15000);
    }
  }
  box.addEventListener("click", function (e) {
    var b = e.target.closest("[data-vote]");
    if (b) { vote(b.dataset.vote); return; }
    if (e.target.closest("#vx-feedback-close")) { remember("dismissed"); hide(); }
  });
})();

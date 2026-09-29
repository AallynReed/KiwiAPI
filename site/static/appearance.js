/* ===========================================================================
   appearance.js - the navbar's Appearance menu, on every page.

   Two choices, each kept in a first-party cookie the server reads for the first
   paint (app/web/appearance.py), so the right look arrives without a flash:
     btt_design  voxel | classic         (switching reloads the page)
     btt_theme   auto | light | dark     (Voxel homepage; applied live)
   Nothing is sent anywhere. window.BTTAppearance lets the homepage's own theme
   control and the design-feedback prompt share the same setters.
   ========================================================================== */
(function () {
  "use strict";

  var YEAR = 31536000;
  var THEMES = ["auto", "light", "dark"];

  function getCookie(name) {
    var m = document.cookie.match(new RegExp("(?:^|; )" + name + "=([^;]*)"));
    return m ? m[1] : "";
  }
  function setCookie(name, value) {
    document.cookie = name + "=" + value + "; Path=/; Max-Age=" + YEAR + "; SameSite=Lax" +
      (location.protocol === "https:" ? "; Secure" : "");
  }
  function theme() {
    var t = getCookie("btt_theme");
    return THEMES.indexOf(t) > -1 ? t : "auto";
  }
  function design() {
    return getCookie("btt_design") === "classic" ? "classic" : "voxel";
  }

  function sync() {
    var t = theme(), d = design();
    document.querySelectorAll("input[data-btt-theme]").forEach(function (i) { i.checked = i.value === t; });
    document.querySelectorAll("input[data-btt-design]").forEach(function (i) { i.checked = i.value === d; });
  }
  function setTheme(t) {
    if (THEMES.indexOf(t) < 0) return;
    setCookie("btt_theme", t);
    sync();
    document.dispatchEvent(new CustomEvent("btt-theme-changed", { detail: { theme: t } }));
  }
  function setDesign(d) {
    if (d !== "voxel" && d !== "classic") return;
    setCookie("btt_design", d);
    location.reload();
  }

  window.BTTAppearance = { theme: theme, design: design, setTheme: setTheme, setDesign: setDesign };

  function init() {
    document.addEventListener("change", function (e) {
      var i = e.target;
      if (!i || i.type !== "radio" || !i.checked) return;
      if (i.hasAttribute("data-btt-theme")) setTheme(i.value);
      else if (i.hasAttribute("data-btt-design")) setDesign(i.value);
    });
    sync();

    var trigger = document.getElementById("nav-appearance-trigger");
    var panel = document.getElementById("nav-appearance-panel");
    if (!trigger || !panel) return;
    function setOpen(open, refocus) {
      panel.hidden = !open;
      trigger.setAttribute("aria-expanded", open ? "true" : "false");
      if (open) {
        var first = panel.querySelector("input:checked") || panel.querySelector("input");
        if (first) first.focus();
      } else if (refocus) {
        trigger.focus();
      }
    }
    trigger.addEventListener("click", function () { setOpen(panel.hidden, false); });
    document.addEventListener("click", function (e) {
      if (!panel.hidden && !e.target.closest("#nav-appearance")) setOpen(false, false);
    });
    document.addEventListener("keydown", function (e) {
      if (e.key === "Escape" && !panel.hidden) setOpen(false, true);
    });
    panel.addEventListener("focusout", function (e) {
      if (e.relatedTarget && !e.relatedTarget.closest("#nav-appearance")) setOpen(false, false);
    });
  }

  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", init);
  else init();
})();

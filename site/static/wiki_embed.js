/* Inline previews on the wiki: a [data-embed] button loads the main site's viewer
   (3D model or visual effect) into its [data-embed-slot] only when asked, so a page
   with dozens of effects costs nothing until someone clicks. */
(function () {
  "use strict";
  document.addEventListener("click", function (ev) {
    const btn = ev.target.closest("[data-embed]");
    if (!btn) return;
    const scope = btn.closest("[data-embed-scope]") || document;
    const slot = scope.querySelector("[data-embed-slot]");
    if (!slot) return;
    const src = btn.getAttribute("data-embed");
    const open = slot.dataset.src === src && !slot.hidden;
    scope.querySelectorAll("[data-embed]").forEach((b) => b.setAttribute("aria-pressed", "false"));
    if (open) {
      slot.hidden = true;
      slot.replaceChildren();
      slot.dataset.src = "";
      return;
    }
    const frame = document.createElement("iframe");
    frame.src = src;
    frame.title = btn.getAttribute("data-embed-title") || btn.textContent.trim();
    frame.loading = "lazy";
    frame.allow = "fullscreen";
    frame.referrerPolicy = "strict-origin-when-cross-origin";
    slot.replaceChildren(frame);
    slot.dataset.src = src;
    slot.hidden = false;
    btn.setAttribute("aria-pressed", "true");
  });

  // A model the renderer can't draw answers 404/422: drop the picture rather than
  // show a broken image. Capture phase, since image errors don't bubble; the sweep
  // covers any that failed before this script ran.
  const hide = (img) => {
    const box = img.closest(".wk-entitycard-img");
    if (box) box.classList.add("is-empty");
    img.remove();
  };
  document.addEventListener("error", (ev) => {
    if (ev.target instanceof HTMLImageElement && ev.target.hasAttribute("data-hide-on-error")) hide(ev.target);
  }, true);
  document.querySelectorAll("img[data-hide-on-error][src]").forEach((img) => {
    if (img.complete && img.naturalWidth === 0) hide(img);
  });

  // Index thumbnails load only as they near the viewport. Native loading="lazy"
  // fetches a screenful or two ahead, which on a 1,200-card list is hundreds at once.
  const lazy = document.querySelectorAll("img[data-lazy-src]");
  const load = (img) => { img.src = img.getAttribute("data-lazy-src"); img.removeAttribute("data-lazy-src"); };
  if (!("IntersectionObserver" in window)) { lazy.forEach(load); return; }
  const io = new IntersectionObserver((seen) => seen.forEach((e) => {
    if (e.isIntersecting) { io.unobserve(e.target); load(e.target); }
  }), { rootMargin: "200px 0px" });
  lazy.forEach((img) => io.observe(img));
})();

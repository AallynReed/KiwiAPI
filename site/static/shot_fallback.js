/* Placeholder tile for feature-card screenshots that haven't been captured
   yet (the <img> 404s). Delegated from document in the CAPTURE phase because
   an <img> `error` event does not bubble - capture is what lets one handler
   replace the 14 per-element `onerror=` attributes the CSP no longer allows.

   Loaded blocking in <head> so the listener is registered before the body's
   images are parsed. Opt-in per image via `data-shot-fallback`. */
(function () {
    'use strict';

    function placeholder(img) {
        const file = (img.getAttribute('src') || '').split('/').pop();
        const ph = document.createElement('div');
        ph.className = 'shot-placeholder';
        const icon = document.createElement('i');
        icon.className = 'fa-regular fa-image';
        icon.setAttribute('aria-hidden', 'true');
        const label = document.createElement('span');
        label.setAttribute('data-i18n', '');
        label.textContent = img.getAttribute('alt') || 'Screenshot';
        const code = document.createElement('code');
        code.textContent = file;
        ph.append(icon, label, code);
        img.replaceWith(ph);
    }

    document.addEventListener('error', function (e) {
        const el = e.target;
        if (el instanceof HTMLImageElement && el.hasAttribute('data-shot-fallback')) {
            placeholder(el);
        }
    }, true);
})();

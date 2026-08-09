/* "Copy link" button on the support page. Deferred, so the DOM is parsed by
   the time this runs. Lifted out of an inline <script> in support.html when
   the site CSP dropped 'unsafe-inline'. */
(function () {
    var btn = document.getElementById('sp-share-copy');
    if (!btn) return;
    btn.addEventListener('click', function () {
        var url = btn.getAttribute('data-copy');
        var label = btn.querySelector('strong');
        var done = function () {
            if (!label || btn.classList.contains('copied')) return;
            var old = label.textContent;
            btn.classList.add('copied');
            label.textContent = 'Link copied!';
            setTimeout(function () {
                btn.classList.remove('copied');
                label.textContent = old;
            }, 1400);
        };
        if (navigator.clipboard && navigator.clipboard.writeText) {
            navigator.clipboard.writeText(url).then(done).catch(function () { fallback(url, done); });
        } else {
            fallback(url, done);
        }
    });
    function fallback(text, cb) {
        var ta = document.createElement('textarea');
        ta.value = text; ta.style.position = 'fixed'; ta.style.opacity = '0';
        document.body.appendChild(ta); ta.focus(); ta.select();
        try { document.execCommand('copy'); cb(); } catch (e) { /* clipboard unavailable */ }
        document.body.removeChild(ta);
    }
})();

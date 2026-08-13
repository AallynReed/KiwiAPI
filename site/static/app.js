document.addEventListener("DOMContentLoaded", () => {

    // Mobile hamburger nav. A resize back to desktop closes it so a stale
    // "open" state doesn't survive orientation flips.
    const navbarEl  = document.querySelector('.navbar');
    const navToggle = document.getElementById('nav-toggle');
    const navLinks  = document.getElementById('nav-links');

    function setNavOpen(open) {
        if (!navbarEl || !navToggle) return;
        navbarEl.classList.toggle('open', open);
        navToggle.setAttribute('aria-expanded', open ? 'true' : 'false');
        // Swap hamburger glyph for an "X" so the same button reads as dismiss.
        const icon = navToggle.querySelector('i');
        if (icon) {
            icon.classList.toggle('fa-bars', !open);
            icon.classList.toggle('fa-xmark', open);
        }
    }
    if (navToggle) {
        navToggle.addEventListener('click', (e) => {
            e.preventDefault();
            e.stopPropagation();
            setNavOpen(!navbarEl.classList.contains('open'));
        });
    }
    if (navLinks) {
        // Tapping any link closes the panel (so an in-page anchor jump doesn't
        // leave it hanging).
        navLinks.addEventListener('click', (e) => {
            const a = e.target.closest('a');
            if (a) setNavOpen(false);
        });
    }
    document.addEventListener('click', (e) => {
        if (!navbarEl || !navbarEl.classList.contains('open')) return;
        if (!navbarEl.contains(e.target)) setNavOpen(false);
    });
    document.addEventListener('keydown', (e) => {
        if (e.key === 'Escape' && navbarEl && navbarEl.classList.contains('open')) {
            setNavOpen(false);
            navToggle?.focus();
        }
    });
    window.addEventListener('resize', () => {
        if (window.innerWidth > 768 && navbarEl && navbarEl.classList.contains('open')) {
            setNavOpen(false);
        }
    });

    // Nav dropdowns ("Developers", "Pages"). Opening one auto-closes the
    // others (single-dropdown UX).
    function closeAllDropdowns() {
        document.querySelectorAll('.nav-dropdown.open').forEach((dd) => {
            dd.classList.remove('open');
            const trig = dd.querySelector('.nav-dropdown-trigger');
            if (trig) trig.setAttribute('aria-expanded', 'false');
        });
        resetPagesFilter();
    }
    document.querySelectorAll('.nav-dropdown').forEach((dd) => {
        const trigger = dd.querySelector('.nav-dropdown-trigger');
        const panel   = dd.querySelector('.nav-dropdown-panel');
        if (!trigger || !panel) return;
        trigger.addEventListener('click', (e) => {
            e.preventDefault();
            e.stopPropagation();
            const willOpen = !dd.classList.contains('open');
            closeAllDropdowns();
            if (willOpen) {
                dd.classList.add('open');
                trigger.setAttribute('aria-expanded', 'true');
            }
        });
        panel.addEventListener('click', (e) => {
            if (e.target.closest('a')) closeAllDropdowns();
        });
    });
    document.addEventListener('click', (e) => {
        if (!e.target.closest('.nav-dropdown')) closeAllDropdowns();
    });
    document.addEventListener('keydown', (e) => {
        if (e.key === 'Escape') {
            const open = document.querySelector('.nav-dropdown.open');
            if (open) {
                closeAllDropdowns();
                open.querySelector('.nav-dropdown-trigger')?.focus();
            }
        }
    });

    // ── Pages menu: type-to-filter + Ctrl/Cmd+K ──────────────────────────
    // The menu carries ~27 destinations and grows by one row per feature, so
    // the columns alone stopped scaling. Filtering reuses the rendered links
    // rather than a parallel index: labels are swapped in place by i18n.js, so
    // reading textContent at match time means search works in every language
    // for free and can never drift out of sync with what's on screen.
    const pagesPanel   = document.getElementById('nav-pages-panel');
    const pagesSearch  = document.getElementById('nav-pages-search');
    const pagesEmpty   = document.getElementById('nav-pages-empty');
    const pagesTrigger = document.getElementById('nav-pages-trigger');

    // Declared as a hoisted function so closeAllDropdowns above can call it
    // regardless of which block runs first.
    function resetPagesFilter() {
        if (!pagesPanel || !pagesSearch) return;
        pagesSearch.value = '';
        pagesPanel.classList.remove('filtering');
        pagesPanel.querySelectorAll('a[hidden]').forEach((a) => { a.hidden = false; });
        pagesPanel.querySelectorAll('.nav-mega-group[hidden]').forEach((g) => { g.hidden = false; });
        pagesPanel.querySelectorAll('.nav-mega-cursor').forEach((a) => a.classList.remove('nav-mega-cursor'));
        if (pagesEmpty) pagesEmpty.hidden = true;
    }

    if (pagesPanel && pagesSearch) {
        const pageLinks = Array.from(pagesPanel.querySelectorAll('.nav-mega-group a'));
        const groups    = Array.from(pagesPanel.querySelectorAll('.nav-mega-group'));

        // The hint ships as "Ctrl K" so it's right without JS; Mac gets the key
        // it actually presses. Not translated - these are key names, not words.
        const kbdHint = pagesPanel.querySelector('.nav-mega-kbd');
        if (kbdHint && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent)) {
            kbdHint.textContent = '⌘ K';
        }

        // Fold case and strip diacritics so "activite" finds "Activité" - the
        // localised labels are what a non-English visitor is typing against.
        const fold = (s) => s.toLowerCase().normalize('NFD').replace(/\p{Diacritic}/gu, '');

        const visibleLinks = () => pageLinks.filter((a) => !a.hidden);

        function moveCursor(step) {
            const links = visibleLinks();
            if (!links.length) return;
            const current = links.findIndex((a) => a.classList.contains('nav-mega-cursor'));
            const next = current < 0
                ? (step > 0 ? 0 : links.length - 1)
                : (current + step + links.length) % links.length;
            links.forEach((a) => a.classList.remove('nav-mega-cursor'));
            links[next].classList.add('nav-mega-cursor');
            links[next].scrollIntoView({ block: 'nearest' });
        }

        function applyFilter() {
            const q = fold(pagesSearch.value.trim());
            if (!q) { resetPagesFilter(); return; }
            pagesPanel.classList.add('filtering');
            let hits = 0;
            pageLinks.forEach((a) => {
                // The href catches route-shaped queries ("/store", "gem-") that
                // the visible label doesn't contain.
                const hay = fold(a.textContent + ' ' + a.getAttribute('href'));
                const match = hay.includes(q);
                a.hidden = !match;
                a.classList.remove('nav-mega-cursor');
                if (match) hits++;
            });
            groups.forEach((g) => {
                g.hidden = !g.querySelector('a:not([hidden])');
            });
            if (pagesEmpty) pagesEmpty.hidden = hits > 0;
            const first = visibleLinks()[0];
            if (first) first.classList.add('nav-mega-cursor');
        }

        pagesSearch.addEventListener('input', applyFilter);
        // Clicks inside the field must not reach the document handler that
        // closes every dropdown.
        pagesSearch.addEventListener('click', (e) => e.stopPropagation());

        pagesSearch.addEventListener('keydown', (e) => {
            if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
                e.preventDefault();
                moveCursor(e.key === 'ArrowDown' ? 1 : -1);
            } else if (e.key === 'Enter') {
                const target = pagesPanel.querySelector('.nav-mega-cursor') || visibleLinks()[0];
                if (target) { e.preventDefault(); window.location.href = target.href; }
            } else if (e.key === 'Escape' && pagesSearch.value) {
                // First Escape clears the query, second closes the menu (the
                // document-level handler above). Stop it here so one keypress
                // doesn't do both.
                e.stopPropagation();
                resetPagesFilter();
            }
        });

        // Focus the field when the menu opens. This listener is registered
        // after the one that toggles .open, so the class is already correct.
        // Desktop only - autofocusing in the mobile drawer throws up the
        // on-screen keyboard over the menu the user just asked to see.
        pagesTrigger?.addEventListener('click', () => {
            if (window.innerWidth > 768 && pagesTrigger.closest('.nav-dropdown')?.classList.contains('open')) {
                pagesSearch.focus();
            }
        });

        document.addEventListener('keydown', (e) => {
            if (e.key !== 'k' && e.key !== 'K') return;
            if (!(e.metaKey || e.ctrlKey) || e.altKey) return;
            const dd = pagesTrigger?.closest('.nav-dropdown');
            if (!dd) return;
            e.preventDefault();
            if (!dd.classList.contains('open')) {
                closeAllDropdowns();
                dd.classList.add('open');
                pagesTrigger.setAttribute('aria-expanded', 'true');
            }
            pagesSearch.focus();
            pagesSearch.select();
        });
    }

    // Per-platform release fetch + render. Hits the Kiwi API instead of GitHub
    // directly: one call returns every platform's latest build (with walk-back
    // if a release skipped a platform), the server cache absorbs the load, and
    // visitors aren't subject to GitHub's 60/hr unauth rate limit.

    const KIWI_API = 'https://api.aallyn.net';
    const releaseInfo = document.getElementById('release-info');

    // Download dropdown: one trigger that ALWAYS opens a panel with all platforms.
    // Never redirects off-site - when the API fails, the panel shows an inline
    // error + retry so the user stays on this page.
    const ddRoot     = document.getElementById('download-dropdown');
    const ddTrigger  = document.getElementById('download-trigger');
    const ddPanel    = document.getElementById('download-panel');
    const ddIcon     = document.getElementById('download-icon');
    const ddLabel    = document.getElementById('download-label');
    const ddStatus   = document.getElementById('download-status');
    const ddStatusMsg= document.getElementById('download-status-msg');
    const ddRetry    = document.getElementById('download-retry');

    let latestData = null;  // cached payload so we can re-render on language change

    const PLATFORM_INFO = {
        windows: { match: /\.(msi|exe)$/i },
        linux:   { match: /\.(appimage|deb|rpm|tar\.gz)$/i },
        android: { match: /\.apk$/i },
    };

    // Localized labels for the release line.
    const RELEASE_I18N = {
        'en':    { latest: 'Latest release', updated: 'updated',           all: 'See all releases on GitHub', locale: 'en' },
        'fr':    { latest: 'Dernière version', updated: 'mis à jour',      all: 'Voir toutes les versions sur GitHub', locale: 'fr' },
        'de':    { latest: 'Neueste Version', updated: 'aktualisiert',     all: 'Alle Versionen auf GitHub ansehen', locale: 'de' },
        'pt-PT': { latest: 'Última versão', updated: 'atualizado',         all: 'Ver todas as versões no GitHub', locale: 'pt-PT' },
        'ru':    { latest: 'Последняя версия', updated: 'обновлено',       all: 'Все версии на GitHub', locale: 'ru' },
        'ja':    { latest: '最新リリース', updated: '更新',                 all: 'GitHub ですべてのリリースを見る', locale: 'ja' },
        'ko':    { latest: '최신 릴리스', updated: '업데이트됨',            all: 'GitHub에서 모든 릴리스 보기', locale: 'ko' },
        'zh-CN': { latest: '最新版本', updated: '更新于',                   all: '在 GitHub 上查看所有版本', locale: 'zh-CN' },
        'es':    { latest: 'Última versión', updated: 'actualizado',        all: 'Ver todas las versiones en GitHub', locale: 'es' },
    };

    function currentLang() {
        try {
            const l = localStorage.getItem('btt_docs_lang');
            if (l && RELEASE_I18N[l]) return l;
        } catch (e) {}
        return (document.documentElement.lang in RELEASE_I18N) ? document.documentElement.lang : 'en';
    }

    // Locale-aware "x hours ago" using Intl.RelativeTimeFormat.
    function timeAgo(iso, locale) {
        const then = new Date(iso).getTime();
        if (isNaN(then)) return '';
        const secs = Math.floor((Date.now() - then) / 1000);
        const units = [['year', 31536000], ['month', 2592000], ['week', 604800],
            ['day', 86400], ['hour', 3600], ['minute', 60], ['second', 1]];
        let rtf;
        try { rtf = new Intl.RelativeTimeFormat(locale, { numeric: 'auto' }); } catch (e) { rtf = new Intl.RelativeTimeFormat('en', { numeric: 'auto' }); }
        for (const [unit, s] of units) {
            const v = Math.floor(secs / s);
            if (v >= 1) return rtf.format(-v, unit);
        }
        return rtf.format(0, 'second');
    }

    // Best-effort browser platform guess for the hero "Download" button.
    function detectPlatform() {
        const ua = (navigator.userAgent || '').toLowerCase();
        if (ua.includes('android')) return 'android';
        if (ua.includes('linux')) return 'linux';
        if (ua.includes('win')) return 'windows';
        return 'windows';  // sensible default for the BTT audience
    }

    // First asset for the platform - the API already sorts by extension
    // priority (.msi before .exe, .AppImage before .deb, etc.), so the first
    // match is the preferred installer.
    function bestAsset(platform, platformData) {
        if (!platformData || !Array.isArray(platformData.assets)) return null;
        const pat = PLATFORM_INFO[platform]?.match;
        if (!pat) return platformData.assets[0] || null;
        return platformData.assets.find(a => pat.test(a.name || '')) || platformData.assets[0] || null;
    }

    function renderDropdownItems() {
        if (!ddPanel || !latestData || !latestData.platforms) return;
        let any = false;
        for (const platform of Object.keys(PLATFORM_INFO)) {
            const item = ddPanel.querySelector(`[data-platform="${platform}"]`);
            if (!item) continue;
            const data = latestData.platforms[platform];
            const asset = bestAsset(platform, data);
            const versionEl = item.querySelector('.download-item-version');
            if (data && asset && data.release && data.release.tag_name) {
                item.href = asset.url;
                item.hidden = false;
                item.title = `${asset.name} - ${data.release.tag_name}`;
                if (versionEl) versionEl.textContent = data.release.tag_name;
                any = true;
            } else {
                item.hidden = true;
            }
        }
        // If nothing's available at all, leave the trigger as-is (still spinning).
        if (!any && ddTrigger) ddTrigger.disabled = true;
    }

    // Flip a .open class on the wrapper so the panel slide + caret rotation
    // kick in via CSS.
    function setDropdownOpen(open) {
        if (!ddRoot || !ddTrigger || !ddPanel) return;
        ddRoot.classList.toggle('open', open);
        ddTrigger.setAttribute('aria-expanded', open ? 'true' : 'false');
        ddPanel.hidden = !open;
        if (open) {
            // Focus the first visible item for keyboard users.
            const first = ddPanel.querySelector('.download-item:not([hidden])');
            if (first) first.focus({ preventScroll: true });
        }
    }
    if (ddTrigger) {
        ddTrigger.addEventListener('click', (e) => {
            e.preventDefault();
            const isOpen = ddRoot.classList.contains('open');
            setDropdownOpen(!isOpen);
        });
    }
    document.addEventListener('click', (e) => {
        if (!ddRoot || !ddRoot.classList.contains('open')) return;
        if (!ddRoot.contains(e.target)) setDropdownOpen(false);
    });
    document.addEventListener('keydown', (e) => {
        if (e.key === 'Escape' && ddRoot && ddRoot.classList.contains('open')) {
            setDropdownOpen(false);
            ddTrigger?.focus();
        }
    });

    function renderReleaseInfo() {
        if (!releaseInfo) return;
        const t = RELEASE_I18N[currentLang()] || RELEASE_I18N.en;
        const userPlat = detectPlatform();
        const platData = latestData && latestData.platforms && latestData.platforms[userPlat];
        const headline = platData?.release;

        if (headline && headline.tag_name && headline.published_at) {
            const exact = new Date(headline.published_at).toLocaleDateString(t.locale, {
                year: 'numeric', month: 'long', day: 'numeric'
            });
            releaseInfo.innerHTML =
                `<i class="fa-solid fa-circle-check ok"></i> ${t.latest} `
                + `<strong>${headline.tag_name}</strong> &middot; ${t.updated} ${timeAgo(headline.published_at, t.locale)}`;
            releaseInfo.title = exact;
            releaseInfo.classList.add('show');
            if (ddTrigger) ddTrigger.title = `${headline.tag_name} - ${exact}`;
        } else {
            // No usable data - leave the release line blank (the dropdown panel
            // owns the user-facing error). Never link off-site to GitHub: that's
            // what the Source Code button is for.
            releaseInfo.innerHTML = '';
            releaseInfo.classList.remove('show');
        }
    }

    // Hide every platform item + the inline error panel (used on retry / fresh
    // load before the API answers).
    function clearPanel() {
        if (!ddPanel) return;
        ddPanel.querySelectorAll('.download-item').forEach(it => { it.hidden = true; });
        if (ddStatus) ddStatus.hidden = true;
        if (ddStatusMsg) ddStatusMsg.textContent = '';
    }

    // Show an error block INSIDE the dropdown panel - never a redirect off-site.
    // The trigger keeps being a dropdown; click again to dismiss, click "Try
    // again" to re-fetch.
    function showPanelError(message) {
        if (!ddStatus || !ddStatusMsg) return;
        ddStatusMsg.textContent = message;
        ddStatus.hidden = false;
    }

    // Move the trigger from "Fetching latest…" to the resting state. The
    // user-platform version is shown on the label when available; otherwise just
    // "Download". The caret stays visible - this is ALWAYS a dropdown trigger.
    function applyTriggerResting() {
        if (!ddIcon || !ddLabel) return;
        ddIcon.classList.remove('fa-spinner', 'fa-spin');
        ddIcon.classList.add('fa-download');
        const userPlat = detectPlatform();
        const data = latestData && latestData.platforms && latestData.platforms[userPlat];
        if (data && data.release && data.release.tag_name) {
            ddLabel.textContent = `Download ${data.release.tag_name}`;
        } else {
            ddLabel.textContent = 'Download';
        }
    }

    // Bound fetch with a hard timeout so a misbehaving proxy or CORS hang
    // can't leave the trigger stuck on "Fetching latest…".
    async function fetchWithTimeout(url, ms) {
        const ctrl = new AbortController();
        const tid = setTimeout(() => ctrl.abort(), ms);
        try { return await fetch(url, { signal: ctrl.signal }); }
        finally { clearTimeout(tid); }
    }

    async function fetchLatest() {
        // Loading state: spinner on the trigger, panel cleared.
        if (ddIcon) { ddIcon.classList.add('fa-spinner', 'fa-spin'); ddIcon.classList.remove('fa-download'); }
        if (ddLabel) ddLabel.textContent = 'Fetching latest…';
        clearPanel();
        try {
            const res = await fetchWithTimeout(
                `${KIWI_API}/v1/btt/latest?channel=release`, 10_000
            );
            if (!res.ok) throw new Error(`HTTP ${res.status}`);
            const data = await res.json();
            // Walk-back already happens server-side: if the latest release had
            // no binaries for a platform, the API returns the previous release
            // that did. So we just take whatever platforms came back.
            const anyBuild = data && data.platforms &&
                Object.values(data.platforms).some(p => p && p.release && p.assets?.length);
            latestData = (data && data.platforms) ? data : null;
            applyTriggerResting();
            renderReleaseInfo();
            if (anyBuild) {
                renderDropdownItems();
            } else {
                // API responded but had no usable builds anywhere - keep the
                // dropdown, show an explanation inside it.
                showPanelError('No builds available right now. Please try again in a moment.');
            }
        } catch (e) {
            console.error('Kiwi API fetch failed:', e);
            latestData = null;
            applyTriggerResting();
            renderReleaseInfo();
            const msg = (e && e.name === 'AbortError')
                ? 'Download list timed out. Check your connection and try again.'
                : 'Could not load the download list. Please try again in a moment.';
            showPanelError(msg);
        }
    }
    fetchLatest();
    if (ddRetry) ddRetry.addEventListener('click', (e) => { e.preventDefault(); fetchLatest(); });
    document.addEventListener('btt-lang-changed', renderReleaseInfo);

    // Smooth-scroll for in-page anchors via scrollIntoView (CSS smooth-scroll
    // is set on <html>). External links and the download-btn id are ignored.
    document.addEventListener('click', (e) => {
        const anchor = e.target.closest('a');
        if (!anchor) return;
        const href = anchor.getAttribute('href');
        if (!href || anchor.target === '_blank' || href.startsWith('http') || anchor.id === 'download-btn') return;
        if (href.startsWith('#') && href.length > 1) {
            const target = document.querySelector(href);
            if (target) {
                e.preventDefault();
                target.scrollIntoView({ behavior: 'smooth', block: 'start' });
            }
        }
    });

    // Support widget (fixed bottom-right donation pill). Lives here, not
    // landing.js, so it wires up on EVERY page that ships the markup - older
    // copies on /leaderboards, /commands, /updates rendered a clickable-looking
    // button that silently did nothing. Tolerant of missing IDs.
    (function () {
        const widget  = document.getElementById('support-widget');
        const trigger = document.getElementById('support-trigger');
        const panel   = document.getElementById('support-panel');
        if (!widget || !trigger || !panel) return;
        // Drop the template's inline `hidden` attribute: a transition from
        // display:none never advances (no computed "before" state), so leaving
        // it on kills the open animation. Closed-state visibility is handled by
        // CSS opacity + pointer-events; aria-hidden below covers screen readers.
        panel.removeAttribute('hidden');
        const setOpen = (open) => {
            widget.classList.toggle('open', open);
            trigger.setAttribute('aria-expanded', open ? 'true' : 'false');
            panel.setAttribute('aria-hidden', open ? 'false' : 'true');
        };
        setOpen(false);
        trigger.addEventListener('click', (e) => {
            e.stopPropagation();
            setOpen(!widget.classList.contains('open'));
        });
        document.addEventListener('click', (e) => {
            if (!widget.contains(e.target) && widget.classList.contains('open')) setOpen(false);
        });
        document.addEventListener('keydown', (e) => {
            if (e.key === 'Escape' && widget.classList.contains('open')) {
                setOpen(false);
                trigger.focus();
            }
        });
    })();
});
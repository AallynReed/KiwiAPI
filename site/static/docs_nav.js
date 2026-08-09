/* Documentation page chrome: mobile sidebar drawer, smooth-scroll nav,
   sidebar search filter, scroll-spy and back-to-top. Lifted out of an inline
   <script> in docs.html when the site CSP dropped 'unsafe-inline'. */
(function () {
    const sidebar   = document.getElementById('sidebar');
    const backdrop  = document.getElementById('backdrop');
    const menuBtn   = document.getElementById('menuBtn');
    const navLinks  = Array.from(document.querySelectorAll('#sideNav a'));
    const search    = document.getElementById('navSearch');
    const navEmpty  = document.getElementById('navEmpty');
    const toTop     = document.getElementById('to-top');

    // ---- Mobile sidebar toggle ----
    const openSidebar  = () => { sidebar.classList.add('open'); backdrop.classList.add('show'); };
    const closeSidebar = () => { sidebar.classList.remove('open'); backdrop.classList.remove('show'); };
    if (menuBtn) menuBtn.addEventListener('click', () => sidebar.classList.contains('open') ? closeSidebar() : openSidebar());
    backdrop.addEventListener('click', closeSidebar);

    // ---- Smooth scroll + close drawer on click ----
    navLinks.forEach(link => {
        link.addEventListener('click', (e) => {
            const id = link.getAttribute('href');
            const target = document.querySelector(id);
            if (target) {
                e.preventDefault();
                target.scrollIntoView({ behavior: 'smooth', block: 'start' });
                history.replaceState(null, '', id);
                closeSidebar();
            }
        });
    });

    // ---- Sidebar search filter ----
    search.addEventListener('input', () => {
        const q = search.value.trim().toLowerCase();
        let anyVisible = false;
        document.querySelectorAll('#sideNav .nav-group').forEach(group => {
            let groupVisible = false;
            group.querySelectorAll('li').forEach(li => {
                const match = li.textContent.toLowerCase().includes(q);
                li.style.display = match ? '' : 'none';
                if (match) { groupVisible = true; anyVisible = true; }
            });
            group.style.display = groupVisible ? '' : 'none';
        });
        navEmpty.style.display = anyVisible ? 'none' : 'block';
    });

    // ---- Scroll-spy (highlight current section) ----
    const sections = navLinks
        .map(l => document.querySelector(l.getAttribute('href')))
        .filter(Boolean);
    const linkFor = (id) => navLinks.find(l => l.getAttribute('href') === '#' + id);

    const spy = new IntersectionObserver((entries) => {
        entries.forEach(entry => {
            if (entry.isIntersecting) {
                const link = linkFor(entry.target.id);
                if (!link) return;
                navLinks.forEach(l => l.classList.remove('active'));
                link.classList.add('active');
            }
        });
    }, { rootMargin: '-80px 0px -65% 0px', threshold: 0 });
    sections.forEach(s => spy.observe(s));

    // ---- Back to top ----
    window.addEventListener('scroll', () => {
        toTop.classList.toggle('show', window.scrollY > 600);
    }, { passive: true });
    toTop.addEventListener('click', () => window.scrollTo({ top: 0, behavior: 'smooth' }));

    // ---- Cmd/Ctrl+K to focus the search ----
    // Standard shortcut on most modern doc sites. On mobile we also open the
    // drawer so the search is visible before we focus it.
    document.addEventListener('keydown', (e) => {
        if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') {
            e.preventDefault();
            if (window.innerWidth <= 860 && !sidebar.classList.contains('open')) openSidebar();
            search.focus();
            search.select();
        }
    });
})();

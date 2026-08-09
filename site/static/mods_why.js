/* Swap the secondary CTA on /mods/why based on login: signed-out -> "Sign in
   to publish", signed-in -> "Create a mod". Deferred and ordered after
   site_auth.min.js, so window.BTTAuth is initialised by the time this runs.
   Lifted out of an inline <script> when the site CSP dropped 'unsafe-inline'. */
(function () {
    var signin = document.getElementById('wmh-signin');
    var create = document.getElementById('wmh-create');

    function apply(user) {
        var authed = !!user;
        if (signin) signin.hidden = authed;
        if (create) create.hidden = !authed;
    }

    var A = window.BTTAuth;
    apply(A && A.getCachedUser ? A.getCachedUser() : null);
    if (A && A.getMe) A.getMe().then(apply).catch(function () {});
})();

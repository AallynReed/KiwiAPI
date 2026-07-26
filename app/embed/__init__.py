"""Embeddable viewers - the Mods Hub's 3D blueprint / creature / VFX previews,
served as a chrome-free page that another site can put in an ``<iframe>``.

  ``app/embed/uploads.py``  ephemeral store for .tmods a partner uploads
  ``app/embed/service.py``  source resolution (release / upload / game file)
  ``app/embed/router.py``   /embed/viewer page + /site/embed/* data + /v1/embed/*

The rendering is the SAME code the hub's own pages use - this module only widens
where the bytes come from. See app/embed/service.py for the source model.
"""

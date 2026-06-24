"""Blueprint → image rendering: a headless, GPU-free voxel rasterizer that
emulates Trove's ``-tool catalog`` output (perspective camera, flat per-face
lighting, glass transparency). See ``voxel.py`` for the renderer and ``service.py``
for the Redis-cached render-by-path entry point."""

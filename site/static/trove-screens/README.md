# Trove screenshots - hero slideshow

Drop screenshots of Trove gameplay here. The landing page picks them up
**dynamically** via `GET /site/screenshots.json` - no HTML/JS edit needed
when you add or remove one.

## Rules

- **Accepted extensions:** `.webp`, `.png`, `.jpg`, `.jpeg`, `.gif`
  (everything else, including this README, is silently skipped)
- **Order:** alphabetical, so prefix files with `01_`, `02_`, … if you want
  to control the cycle order. Otherwise filename sort decides.
- **Resolution:** aim for ≥1920×1080. The hero uses `object-fit: cover`, so
  smaller images will be upscaled; larger images cost bandwidth but display
  the same.
- **Format:** `.webp` is strongly preferred - typically 30–50 % smaller than
  the equivalent `.png` at the same visual quality. Use `cwebp -q 80` from
  the command line, or any half-decent online converter.
- **Aspect:** wide landscape (16:9 or wider) reads best; the hero crops
  centre-cover so tall portraits will be heavily clipped.

## How it shows up

- Stacked behind the glowing orbs + grid + particles in the hero
- Crossfaded every ~8 seconds with a slow ken-burns zoom
- Auto-dimmed to ~30 % opacity so the hero copy stays readable
- Honours `prefers-reduced-motion`: shows the first image and skips the
  rotation entirely

## Caching

The `/site/screenshots.json` endpoint sets `Cache-Control: max-age=60`, so
a freshly-added screenshot appears within a minute (without users having
to hard-refresh). Want it instant? Just bump the cache-buster query
string in `site/templates/index.html`.

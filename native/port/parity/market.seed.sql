-- 25 days of listings for five items, anchored to the current hour so both stacks get identical rows,
-- with a few 400x outliers, concluded and live listings; plus codex names for thumbnails and tome art.
CREATE TABLE IF NOT EXISTS market_listing (
    id          UUID PRIMARY KEY,
    name        TEXT   NOT NULL,
    type        TEXT,
    stack       INTEGER NOT NULL,
    price       BIGINT  NOT NULL,
    price_each  DOUBLE PRECISION NOT NULL,
    last_seen   BIGINT  NOT NULL,
    created_at  BIGINT  NOT NULL DEFAULT 0
);

WITH a AS (SELECT (extract(epoch FROM now())::bigint / 3600) * 3600 AS t),
items(name, kind, base) AS (
    VALUES ('Glim', 'Material', 120), ('Plasmium', 'Material', 900), ('Shapestone Ore', 'Ore', 35),
           ('Eye of Q''bthulhu', NULL, 15000), ('Untracked Thing', 'Misc', 5)
),
g AS (
    SELECT i.name, i.kind, s, 1 + (s * 7) % 9 AS stack,
           (1 + (s * 7) % 9) * i.base * (80 + abs(hashtext(i.name || s)) % 40) / 100
             * CASE WHEN s % 97 = 0 THEN 400 ELSE 1 END AS price,
           a.t - s * 9000 - 600 AS created_at,
           (s * 37 % 200) * 3600 AS lifespan,
           a.t
    FROM items i, a, generate_series(1, 240) s
)
INSERT INTO market_listing (id, name, type, stack, price, price_each, last_seen, created_at)
SELECT md5(name || ':' || s)::uuid, name, kind, stack, price, round(price::numeric / stack, 3)::float8,
       LEAST(t, created_at + lifespan), created_at
FROM g
ON CONFLICT (id) DO NOTHING;

INSERT INTO codex_entry (branch, path, codex_type, content_sha256, name, blueprint) VALUES
    ('live-us', 'prefabs/item/crafting/glim', 'item', 'p1', 'Glim', 'item/glim'),
    ('live-us', 'prefabs/item/crafting/glim_debug', 'item', 'p2', 'Glim', 'item/glim_debug'),
    ('live-us', 'prefabs/collections/plasmium', 'item', 'p3', 'Plasmium', 'collections/plasmium_a'),
    ('live-us', 'prefabs/collections/plasmium2', 'item', 'p4', 'Plasmium', 'collections/plasmium_b'),
    ('live-us', 'prefabs/item/tome/shapestone', 'item', 'p5', 'Tome: Shapestone Survey', 'item/tome_shapestone'),
    ('live-us', 'prefabs/item/tome/glim', 'item', 'p6', 'Glim Grimoire', 'item/TOME_glim'),
    ('live-us', 'prefabs/item/crafting/eye', 'item', 'p7', 'EYE OF Q''BTHULHU', 'item/eye'),
    ('pts', 'prefabs/item/crafting/shapestone', 'item', 'p8', 'Shapestone Ore', 'item/pts_only')
ON CONFLICT DO NOTHING;

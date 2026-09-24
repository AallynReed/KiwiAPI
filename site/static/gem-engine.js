/* =========================================================================
   Better Trove Tools - client-side gem engine
   -------------------------------------------------------------------------
   A faithful JS port of the Python gem model (app/trove/gems/model.py +
   bases.py + constants.py) so the Gem Simulator runs in the browser. The game
   numbers come from /gamedata/gem_upgrades.json: call GemEngine.load() first.

   web_mode.js routes the gem eel-shim functions here. The output matches the
   Python `model_dump(mode='json')` shape exactly, including the computed
   fields the UI reads (value / real_value / increase / augmentation_progress /
   quality / power_rank / stat_values / gem_name / ...). Gem creation is
   inherently random, so Math.random() is used; the DETERMINISTIC computations
   (quality, power rank, stat values) match the Python engine.
   ========================================================================= */
(function () {
    // ---- enum values (must match gem_constants.py) ----
    const Tier = { RADIANT: 1, STELLAR: 2, CRYSTAL: 3, MYSTIC: 4 };
    const Type = { LESSER: 1, EMPOWERED: 2 };
    const Element = { WATER: 1, FIRE: 2, AIR: 3, COSMIC: 4 };
    const Stat = {
        PHYSICAL_DAMAGE: 1, MAGIC_DAMAGE: 2, CRITICAL_DAMAGE: 3, CRITICAL_HIT: 4,
        MAX_HEALTH: 5, MAX_HEALTH_BONUS: 6, LIGHT: 7, MOVEMENT_SPEED: 8, JUMP: 9
    };
    const Restriction = { FIERCE: 1, ARCANE: 2 };
    const Augment = { ROUGH: 1, PRECISE: 2, SUPERIOR: 3 };

    // ---- display names (GEM_*_NAMES) ----
    const TIER_NAMES = { 1: "Radiant", 2: "Stellar", 3: "Crystal", 4: "Mystic" };
    const TYPE_NAMES = { 1: "Lesser", 2: "Empowered" };
    const ELEMENT_NAMES = { 1: "Water", 2: "Fire", 3: "Air", 4: "Cosmic" };
    const STAT_NAMES = {
        1: "Physical Damage", 2: "Magic Damage", 3: "Critical Damage", 4: "Critical Hit",
        5: "Max Health", 6: "Max Health %", 7: "Light", 8: "Movement Speed", 9: "Jump"
    };
    const RESTRICTION_NAMES = { 1: "Fierce", 2: "Arcane" };
    const ABILITY_NAMES = {
        1: "Stinging Curse", 2: "Volatile Velocity", 3: "Spirit Surge", 4: "Mired Mojo",
        5: "Stunburst", 6: "Pyrodisc", 7: "Explosive Epilogue", 8: "Cubic Curtain",
        9: "Berserk Battler", 10: "Empyrean Barrier", 11: "Flower Power", 12: "Vampirian Vanquisher"
    };

    // ---- stat pools / restrictions / abilities (keyed by element value) ----
    const GEM_STAT_RESTRICTIONS = {
        [Element.AIR]: [1, 2, 3, 4, 5, 6],
        [Element.FIRE]: [1, 2, 3, 4, 5, 6],
        [Element.WATER]: [1, 2, 3, 4, 5, 6],
        [Element.COSMIC]: [1, 2, 3, 4, 5, 6, 7]
    };
    const PHYSICAL_GEM_STAT_POOL = {
        [Element.AIR]: [1, 3, 4, 5, 6],
        [Element.FIRE]: [1, 3, 4, 5, 6],
        [Element.WATER]: [1, 3, 4, 5, 6],
        [Element.COSMIC]: [1, 3, 4, 5, 6]
    };
    const MAGIC_GEM_STAT_POOL = {
        [Element.AIR]: [2, 3, 4, 5, 6],
        [Element.FIRE]: [2, 3, 4, 5, 6],
        [Element.WATER]: [2, 3, 4, 5, 6],
        [Element.COSMIC]: [2, 3, 4, 5, 6]
    };
    const GEM_ABILITIES = {
        [Element.WATER]: [1, 2, 3, 4, 5, 6, 7, 8],
        [Element.FIRE]: [1, 2, 3, 4, 5, 6, 7, 8],
        [Element.AIR]: [1, 2, 3, 4, 5, 6, 7, 8],
        [Element.COSMIC]: [9, 10, 11, 12]
    };

    // ---- game data (gem_upgrades.json; app/trove/gems/bases.py is the Python twin) ----
    // Stat rolls, the level schedule, level-up odds/costs, focuses and boosters come
    // from the game files. Power Rank is not in them and stays as BTT had it.
    let DATA = null;
    let INDEX = {};
    const PR_BASE = { 1: 3, 2: 5, 3: 7, 4: 9 };
    const LESSER_PR_THRESHOLD = { 1: [85, 113], 2: [150, 200], 3: [175, 250], 4: [200, 260] };
    const EMPOWERED_PR_THRESHOLD = { 1: [113, 150], 2: [200, 266], 3: [220, 280], 4: [240, 300] };
    const FILE_TIER = { 1: 9, 2: 10, 3: 11, 4: 12 };
    const COLOR = { 1: "blue", 2: "red", 3: "yellow", 4: "opal" };
    const STAT_KEY = {
        1: "PhysicalDamage|false", 2: "SpellDamage|false", 3: "CriticalHitDamage|false",
        4: "CriticalHitChance|false", 5: "MaxHealth|false", 6: "MaxHealth|true", 7: "Light|false"
    };
    const FOCUS = { 1: "item/gem/booster/augment1", 2: "item/gem/booster/augment2", 3: "item/gem/booster/augment3" };

    function setData(data) {
        DATA = data || { gems: [] };
        INDEX = {};
        for (const g of DATA.gems || []) for (const i of g.items) INDEX[i.size + "/" + i.color + "/" + i.tier] = g;
    }
    function upgradeData(tier, type, element) {
        const size = type === Type.LESSER ? "small" : "large";
        const t = FILE_TIER[tier] + (type === Type.LESSER ? 0 : 100);
        return INDEX[size + "/" + COLOR[element || Element.WATER] + "/" + t] || null;
    }
    const itemName = (item) => (DATA && DATA.names && DATA.names[item]) || item.split("/").pop();
    const levelAttempt = (tier, type, element, level) => {
        const g = upgradeData(tier, type, element);
        return g ? (g.levels.find(l => l.level === level) || null) : null;
    };
    function getGemMaxLevel(tier, type) {
        const g = upgradeData(tier, type || Type.LESSER, Element.WATER);
        return g ? g.max_level : null;
    }
    function boostLevels(tier, type) {
        const g = upgradeData(tier, type || Type.LESSER, Element.WATER);
        return g ? g.levels.filter(l => l.boost).map(l => l.level) : [];
    }
    const boostsAt = (tier, type, level) => boostLevels(tier, type).filter(l => l <= level).length;
    function getIncrementPowerRank(tier, level) {
        const a = levelAttempt(tier, Type.LESSER, Element.WATER, level);
        return PR_BASE[tier] * (a ? a.stat_steps : 0);
    }
    function statRoll(tier, type, element, statType) {
        const g = upgradeData(tier, type, element);
        const key = STAT_KEY[statType];
        return g ? (g.stats.find(r => r.stat + "|" + r.percent === key) || null) : null;
    }
    function getStatBase(tier, type, element, statType) {
        const r = statRoll(tier, type, element, statType);
        return r ? r.step / PR_BASE[tier] : null;
    }
    function getStatThreshold(tier, type, element, statType) {
        const r = statRoll(tier, type, element, statType);
        if (!r) return null;
        const base = r.step / PR_BASE[tier];
        return [r.min / base, r.max / base];
    }
    const getLesserGemPrThreshold = (tier) => LESSER_PR_THRESHOLD[tier] || null;
    const getEmpoweredGemPrThreshold = (tier) => EMPOWERED_PR_THRESHOLD[tier] || null;
    function getAugmentBase(augment) {
        const f = ((DATA && DATA.focuses) || []).find(x => x.item === FOCUS[augment]);
        return f ? f.amount : null;
    }
    function statWeights(tier, type, element) {
        const g = upgradeData(tier, type, element);
        const water = upgradeData(tier, type, Element.WATER);
        const pool = (g && g.pool.length) ? g.pool : (water ? water.pool : []);
        const out = {};
        for (const [st, key] of Object.entries(STAT_KEY)) {
            const p = pool.find(r => r.stat + "|" + r.percent === key);
            if (p) out[st] = p.weight;
        }
        return out;
    }
    function boosters() {
        return ((DATA && DATA.boosters) || []).map(b => ({
            id: b.item.split("/").pop(), item: b.item, name: itemName(b.item),
            chance_multiplier: b.chance_multiplier, double_multiplier: b.double_multiplier
        }));
    }
    const findBooster = (id) => (id ? boosters().find(b => b.id === id) || null : null);
    function attemptOdds(attempt, boost) {
        return [Math.min(1, attempt.chance * (boost ? boost.chance_multiplier : 1)),
                Math.min(1, attempt.double_chance * (boost ? boost.double_multiplier : 1))];
    }
    function attemptCost(element, attempt, boost) {
        const mats = (DATA && DATA.materials && DATA.materials[COLOR[element]]) || {};
        const rows = attempt.cost.map(c => [c.item, c.count]);
        if (attempt.dust && mats.dust) rows.push([mats.dust, attempt.dust]);
        if (attempt.amber && mats.amber) rows.push([mats.amber, attempt.amber]);
        if (boost) rows.push([boost.item, 1]);
        return rows.map(([item, count]) => ({ item, name: itemName(item), count }));
    }

    // ---- helpers ----
    // Python round(): round-half-to-even (banker's rounding).
    function pyRound(x, ndigits) {
        if (ndigits === undefined || ndigits === null) {
            const floor = Math.floor(x);
            const diff = x - floor;
            if (diff < 0.5) return floor;
            if (diff > 0.5) return floor + 1;
            return (floor % 2 === 0) ? floor : floor + 1;
        }
        const m = Math.pow(10, ndigits);
        return pyRound(x * m) / m;
    }
    const choice = (arr) => arr[Math.floor(Math.random() * arr.length)];
    const randint = (a, b) => a + Math.floor(Math.random() * (b - a + 1));
    function weightedSample(arr, weights, k) {
        const left = arr.slice();
        const out = [];
        while (out.length < k && left.length) {
            let w = left.map(s => weights[s] || 0);
            if (!w.some(x => x > 0)) w = left.map(() => 1);
            let r = Math.random() * w.reduce((a, b) => a + b, 0);
            let i = 0;
            while (i < left.length - 1 && r >= w[i]) { r -= w[i]; i++; }
            out.push(left.splice(i, 1)[0]);
        }
        return out;
    }
    let _idCounter = 0;
    const genId = () => Date.now() * 1000 + (_idCounter++ % 1000);

    // ---- raw structure builders (mirror StatContainer / Stat defaults) ----
    function makeContainer(base) {
        return {
            base: (base === undefined || base === null) ? Math.random() : base,
            augments: [
                { type: Augment.ROUGH, count: 0 },
                { type: Augment.PRECISE, count: 0 },
                { type: Augment.SUPERIOR, count: 0 }
            ]
        };
    }
    const makeStat = (type) => ({ type, containers: [], locked: false });

    // ---- derived values (StatContainer / Stat computed fields) ----
    function containerIncrease(c) {
        let inc = 0;
        for (const aug of (c.augments || [])) inc += (getAugmentBase(aug.type) / 100) * aug.count;
        return inc;
    }
    const containerRealValue = (c) => c.base + containerIncrease(c);
    const containerValue = (c) => Math.min(c.base + containerIncrease(c), 1);
    function statAugmentationProgress(stat) {
        let v = 0;
        for (const c of stat.containers) v += containerRealValue(c);
        return Math.min(v / stat.containers.length, 1);
    }

    // ---- serialization (model_dump(mode='json') with computed fields) ----
    function serializeContainer(c) {
        const increase = containerIncrease(c);
        return {
            base: c.base,
            augments: c.augments.map(a => ({ type: a.type, count: a.count })),
            increase: increase,
            value: Math.min(c.base + increase, 1),
            real_value: c.base + increase
        };
    }
    function serializeStat(stat) {
        return {
            type: stat.type,
            containers: stat.containers.map(serializeContainer),
            locked: !!stat.locked,
            augmentation_progress: statAugmentationProgress(stat)
        };
    }
    function gemQuality(gem) {
        let total = 0, count = 0;
        for (const stat of gem.stats) for (const c of stat.containers) { total += containerValue(c); count++; }
        if (count === 0) return 0.0;
        return pyRound((total / count) * 100, 1) / 100;
    }
    function gemPowerRank(gem) {
        let pr = (gem.type === Type.LESSER) ? 0 : 100;
        const thresholds = (gem.type === Type.LESSER)
            ? getLesserGemPrThreshold(gem.tier) : getEmpoweredGemPrThreshold(gem.tier);
        for (const stat of gem.stats) {
            const progress = thresholds[0] + (thresholds[1] - thresholds[0]) * statAugmentationProgress(stat);
            pr += progress * stat.containers.length;
        }
        for (let s = 0; s < gem.stats.length; s++) {
            for (let level = 1; level <= gem.level; level++) pr += getIncrementPowerRank(gem.tier, level);
        }
        return pyRound(pr);
    }
    function gemStatValues(gem) {
        let prIncrements = 0;
        for (let level = 1; level <= gem.level; level++) prIncrements += getIncrementPowerRank(gem.tier, level);
        const out = [];
        for (const stat of gem.stats) {
            const statBase = getStatBase(gem.tier, gem.type, gem.element, stat.type);
            const thresholds = getStatThreshold(gem.tier, gem.type, gem.element, stat.type);
            const progress = thresholds[0] + (thresholds[1] - thresholds[0]) * statAugmentationProgress(stat);
            let statValue = statBase * progress * stat.containers.length;
            statValue += statBase * prIncrements;
            out.push({ [STAT_NAMES[stat.type]]: statValue });
        }
        return out;
    }
    function gemName(gem) {
        if (gem.type === Type.LESSER) {
            return `${RESTRICTION_NAMES[gem.restriction]} ${TIER_NAMES[gem.tier]} Gem`;
        }
        return gem.ability ? ABILITY_NAMES[gem.ability] : null;
    }
    function serializeGem(gem) {
        const maxLevel = getGemMaxLevel(gem.tier, gem.type);
        return {
            id: gem.id,
            tier: gem.tier,
            type: gem.type,
            element: gem.element,
            restriction: (gem.restriction === undefined ? null : gem.restriction),
            ability: (gem.ability === undefined ? null : gem.ability),
            level: gem.level,
            stats: gem.stats.map(serializeStat),
            augmentation: (gem.augmentation === undefined ? null : gem.augmentation),
            attempts: gem.attempts || 0,
            spent: Object.assign({}, gem.spent || {}),
            ability_name: gem.ability ? ABILITY_NAMES[gem.ability] : null,
            gem_name: gemName(gem),
            is_max_level: gem.level === maxLevel,
            quality: gemQuality(gem),
            power_rank: gemPowerRank(gem),
            stat_values: gemStatValues(gem)
        };
    }

    // ---- normalize an incoming (possibly serialized) gem to raw structure ----
    function normalize(g) {
        return {
            id: (g.id !== undefined ? g.id : genId()),
            tier: g.tier, type: g.type, element: g.element,
            restriction: (g.restriction === undefined ? null : g.restriction),
            ability: (g.ability === undefined ? null : g.ability),
            level: g.level,
            augmentation: (g.augmentation === undefined ? null : g.augmentation),
            attempts: g.attempts || 0,
            spent: Object.assign({}, g.spent || {}),
            stats: (g.stats || []).map(s => ({
                type: s.type,
                locked: !!s.locked,
                containers: (s.containers || []).map(c => ({
                    base: c.base,
                    augments: (c.augments && c.augments.length)
                        ? c.augments.map(a => ({ type: a.type, count: a.count || 0 }))
                        : [{ type: 1, count: 0 }, { type: 2, count: 0 }, { type: 3, count: 0 }]
                }))
            }))
        };
    }

    // ---- operations (mirror gem_simulator.py) ----
    function create(data) {
        data = data || {};
        const augLevel = (data.augmentation === undefined || data.augmentation === null) ? undefined : data.augmentation;
        let tier = data.tier || choice([1, 2, 3, 4]);
        let type = data.type || choice([1, 2]);
        let element = data.element ? data.element : choice([1, 2, 3, 4]);
        let level = data.level || 1;
        let restriction;
        if (type === Type.LESSER) {
            restriction = (data.restriction === undefined || data.restriction === null)
                ? choice([Restriction.FIERCE, Restriction.ARCANE]) : data.restriction;
        } else {
            restriction = null;
        }
        const extraContainers = boostsAt(tier, type, level);

        let stats;
        const pool = (restriction === null || restriction === undefined)
            ? choice([PHYSICAL_GEM_STAT_POOL, MAGIC_GEM_STAT_POOL])
            : (restriction === Restriction.FIERCE ? PHYSICAL_GEM_STAT_POOL : MAGIC_GEM_STAT_POOL);
        const statTypes = weightedSample(pool[element], statWeights(tier, type, element), 3);
        stats = statTypes.map(makeStat);
        if (element === Element.COSMIC) {
            const index = randint(0, 2);
            stats[index].type = Stat.LIGHT;
            stats[index].locked = true;
        }
        for (const stat of stats) stat.containers.push(makeContainer(augLevel));
        for (let i = 0; i < extraContainers; i++) {
            const index = randint(0, 2);
            stats[index].containers.push(makeContainer(augLevel));
        }
        const maxLevel = getGemMaxLevel(tier, type);
        level = Math.min(level, maxLevel);
        const ability = (type === Type.EMPOWERED) ? choice(GEM_ABILITIES[element]) : null;
        return {
            id: genId(), tier, type, element, restriction, ability, level,
            augmentation: (augLevel === undefined ? null : augLevel), stats, attempts: 0, spent: {}
        };
    }

    const augLevelOf = (gem) => (gem.augmentation === undefined || gem.augmentation === null) ? undefined : gem.augmentation;
    const hasStat = (gem, statType) => gem.stats.some(s => s.type === statType);

    function gainLevel(gem) {
        gem.level += 1;
        if (boostLevels(gem.tier, gem.type).includes(gem.level)) {
            gem.stats[randint(0, 2)].containers.push(makeContainer(augLevelOf(gem)));
        }
    }
    // One level-up attempt at the game's odds (mirrors Gem.level_up): the materials
    // and booster are spent whether or not it lands; a double gains two levels.
    function levelUp(gem, boosterId) {
        const attempt = levelAttempt(gem.tier, gem.type, gem.element, gem.level + 1);
        if (!attempt) return null;
        const boost = findBooster(boosterId);
        const [chance, double] = attemptOdds(attempt, boost);
        const cost = attemptCost(gem.element, attempt, boost);
        gem.attempts = (gem.attempts || 0) + 1;
        gem.spent = gem.spent || {};
        for (const c of cost) gem.spent[c.name] = (gem.spent[c.name] || 0) + c.count;
        let outcome = "failed";
        if (Math.random() < chance) {
            const gained = (Math.random() < double && gem.level + 2 <= getGemMaxLevel(gem.tier, gem.type)) ? 2 : 1;
            for (let i = 0; i < gained; i++) gainLevel(gem);
            outcome = gained === 2 ? "double" : "success";
        }
        return { outcome, chance, double_chance: double, cost };
    }
    function addAugmentToStat(stat, augmentType) {
        if (statAugmentationProgress(stat) === 1) return false;
        for (const c of stat.containers) {
            if (containerRealValue(c) >= 1) continue;
            const a = c.augments.find(x => x.type === augmentType);
            if (a) a.count += 1;
            return true;
        }
        return false;
    }
    function rerollStatType(gem, statType) {
        const inUse = gem.stats.map(s => s.type);
        if (!hasStat(gem, statType)) return false;
        for (const stat of gem.stats) {
            if (stat.type === statType && !stat.locked) {
                const pool = gem.restriction
                    ? (gem.restriction === Restriction.FIERCE ? PHYSICAL_GEM_STAT_POOL : MAGIC_GEM_STAT_POOL)
                    : GEM_STAT_RESTRICTIONS;
                const statTypes = pool[gem.element];
                let unused = statTypes.filter(s => !inUse.includes(s));
                if (inUse.includes(Stat.PHYSICAL_DAMAGE)) unused = unused.filter(s => s !== Stat.MAGIC_DAMAGE);
                if (inUse.includes(Stat.MAGIC_DAMAGE)) unused = unused.filter(s => s !== Stat.PHYSICAL_DAMAGE);
                if (!unused.length) return false;
                stat.type = choice(unused);
                return true;
            }
        }
        return false;
    }
    function moveProc(gem, statType) {
        if (!hasStat(gem, statType)) return false;
        for (const stat of gem.stats) {
            if (stat.type === statType) {
                if (stat.containers.length === 1) return false;
                const last = stat.containers.pop();
                const others = gem.stats.filter(s => s.type !== statType);
                choice(others).containers.push(last);
                return true;
            }
        }
        return false;
    }

    // ---- public API: returns the same resp-shape as the Python eel functions ----
    const ok = (gem) => ({ success: true, data: { gem }, gem });
    const err = (error, code) => ({ success: false, error, code });

    function getLookups() {
        // name-based titles, sorted by value (matches get_gem_lookups)
        return {
            success: true,
            data: {
                types: { "Lesser": 1, "Empowered": 2 },
                elements: { "Water": 1, "Fire": 2, "Air": 3, "Cosmic": 4 },
                tiers: { "Radiant": 1, "Stellar": 2, "Crystal": 3, "Mystic": 4 },
                restrictions: { "Fierce": 1, "Arcane": 2 },
                stat_types: {
                    "Physical Damage": 1, "Magic Damage": 2, "Critical Damage": 3, "Critical Hit": 4,
                    "Max Health": 5, "Max Health Bonus": 6, "Light": 7, "Movement Speed": 8, "Jump": 9
                },
                augment_types: { "Rough": 1, "Precise": 2, "Superior": 3 }
            }
        };
    }

    window.GemEngine = {
        getLookups,
        createGem(data) {
            const gem = create(data || {});
            return ok(serializeGem(gem));
        },
        // Fetches the game data; resolves once the engine can be used.
        load(url) {
            return fetch(url || "/gamedata/gem_upgrades.json", { credentials: "same-origin" })
                .then(r => { if (!r.ok) throw new Error("HTTP " + r.status); return r.json(); })
                .then(setData);
        },
        setData,
        boosters,
        itemName,
        // The next attempt's odds and cost, for the UI (null at max level).
        nextAttempt(gemData, boosterId) {
            const gem = normalize(gemData);
            const attempt = levelAttempt(gem.tier, gem.type, gem.element, gem.level + 1);
            if (!attempt) return null;
            const boost = findBooster(boosterId);
            const [chance, double] = attemptOdds(attempt, boost);
            return { level: attempt.level, chance, double_chance: double, cost: attemptCost(gem.element, attempt, boost) };
        },
        levelUpGem(gemData, boosterId) {
            const gem = normalize(gemData);
            const out = levelUp(gem, boosterId);
            if (!out) return err("Gem is already at max level.", "GEM_MAX_LEVEL");
            return Object.assign(ok(serializeGem(gem)), out);
        },
        augmentGem(gemData, statId, augmentId) {
            const gem = normalize(gemData);
            if (!hasStat(gem, statId)) return err("Stat type not found in gem", "GEM_STAT_NOT_FOUND");
            for (const stat of gem.stats) {
                if (stat.type === statId) {
                    if (!addAugmentToStat(stat, augmentId)) return err("Stat is already fully augmented", "GEM_STAT_MAX_AUGMENT");
                    return ok(serializeGem(gem));
                }
            }
            return err("Failed to augment", "GEM_AUGMENT_FAILED");
        },
        sparkGem(gemData, statId) {
            const gem = normalize(gemData);
            if (!hasStat(gem, statId)) return err("Stat type not found in gem", "GEM_STAT_NOT_FOUND");
            if (!rerollStatType(gem, statId)) return err("Failed to reroll stat. It might be locked.", "GEM_REROLL_FAILED");
            return ok(serializeGem(gem));
        },
        flareGem(gemData, statId) {
            const gem = normalize(gemData);
            if (!hasStat(gem, statId)) return err("Stat type not found in gem", "GEM_STAT_NOT_FOUND");
            if (!moveProc(gem, statId)) return err("Cannot move boost from a stat with only one proc.", "GEM_MOVE_PROC_FAILED");
            return ok(serializeGem(gem));
        },
        massUpdate(gems) {
            const out = (gems || []).map(g => (g === null || g === undefined) ? null : serializeGem(normalize(g)));
            return { success: true, data: { gems: out }, gems: out };
        }
    };
})();

/* ═══════════════════════════════════════════════════════════════════════
   /fishing-guide - the interactive Trove fishing guide.
   Vanilla JS, no deps, CSP-clean, client-only bar the fish thumbnails, which
   are blueprint renders from the codex (/site/codexes/render) and simply
   don't appear if that read fails.

   The catch conditions come from the guide this page was written from; the
   names, rarities and blueprints are joined onto the live fish codex, which
   is the authority for those. `FISH` rows are
   [name, blueprint, liquid, rarity, source, poolGroup, rod, condition].
   ═══════════════════════════════════════════════════════════════════════ */
(function () {
  "use strict";
  var doc = document, body = doc.body;
  var REDUCE = window.matchMedia && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  if (REDUCE) body.classList.add("fg-reduce");

  /* ── tiny DOM helper (same shape as the gems guide's) ───────────────── */
  function el(tag, attrs, kids) {
    var n = doc.createElement(tag), k;
    if (attrs) for (k in attrs) {
      if (k === "class") n.className = attrs[k];
      else if (k === "html") n.innerHTML = attrs[k];
      else if (k === "text") n.textContent = attrs[k];
      else if (k.slice(0, 5) === "data-" || k.slice(0, 5) === "aria-" || k === "role" ||
               k === "type" || k === "loading" || k === "alt" || k === "src") n.setAttribute(k, attrs[k]);
      else n[k] = attrs[k];
    }
    if (kids != null) (Array.isArray(kids) ? kids : [kids]).forEach(function (c) {
      if (c != null) n.appendChild(typeof c === "string" ? doc.createTextNode(c) : c);
    });
    return n;
  }
  function $(sel, ctx) { return (ctx || doc).querySelector(sel); }
  function clear(n) { while (n.firstChild) n.removeChild(n.firstChild); return n; }

  /* ── i18n for JS-built strings (the [data-i18n] sweep only covers markup) */
  function tt(s) { return (window.BTTi18n && window.BTTi18n.t) ? window.BTTi18n.t(s) : s; }
  function ttf(s, map) { return tt(s).replace(/\{(\w+)\}/g, function (_, k) { return (map && map[k] != null) ? map[k] : "{" + k + "}"; }); }
  // Marks a literal that lives in a data table and is looked up later through
  // tt(): the extractor only sees literals at a call site, so without this the
  // whole reference layer below reads as untranslatable. Returns the string
  // unchanged - the real lookup still happens at render time.
  function tr(s) { return s; }
  var RERENDER = [];
  function onLang(fn) { RERENDER.push(fn); }
  doc.addEventListener("btt-lang-changed", function () { RERENDER.forEach(function (f) { try { f(); } catch (e) {} }); });

  /* ── reference data ─────────────────────────────────────────────────── */
  var LIQUIDS = [
    { key: "water", name: tr("Water"), color: "#58a6ff", icon: "fa-droplet" },
    { key: "lava", name: tr("Lava"), color: "#ff7a45", icon: "fa-fire" },
    { key: "choc", name: tr("Chocolate"), color: "#d08c5a", icon: "fa-mug-hot" },
    { key: "plasma", name: tr("Plasma"), color: "#2fd4e6", icon: "fa-bolt" }
  ];
  // Rarity is the codex's, and its colour is the one the game rings a fish
  // with: grey, green, blue.
  var RARITY = [
    { name: tr("Common"), color: "#c6cfdb", mastery: 5 },
    { name: tr("Uncommon"), color: "#3fb950", mastery: 10 },
    { name: tr("Rare"), color: "#58a6ff", mastery: 25 }
  ];
  // Index 0 is "no special rod" - the value a fish row stores.
  var RODS = [
    null,
    { name: "Lady of the Lake", short: "Lady of the Lake" },
    { name: "Murkwater Mark's Mucker", short: "Mucker" },
    { name: "Turtle Trawler", short: "Turtle Trawler" }
  ];
  var SRC = { OPEN: 0, POOL: 1, SPECIAL: 2, TURTLE: 3, EXTRA: 4 };

  var FISH = [
    ["Jumping Jadefin","fish_water_uncommon.blueprint",0,0,0,0,0,0],
    ["Blue Balladine","fish_water_rare.blueprint",0,0,0,0,0,0],
    ["Violet Verseskimmer","fish_water_epic.blueprint",0,0,0,0,0,0],
    ["Genteel Goldfish","fish_water_legendary.blueprint",0,0,0,0,0,0],
    ["Crimson Siltdancer","fish_water_relic.blueprint",0,0,0,0,0,0],
    ["Thalasstian Princess","fish_water_resplendent.blueprint",0,0,0,0,0,0],
    ["Abyssal Gazer","fish_water_shadow.blueprint",0,0,0,0,0,0],
    ["Fat Catfish","fish_fatcat.blueprint",0,1,0,0,0,0],
    ["Radiant Shardine","fish_shardine_radiant.blueprint",0,1,0,0,0,0],
    ["Frozen Orefish","fish_water_iceore.blueprint",0,1,0,0,0,0],
    ["Saltwater Swordfish","fish_swordfish.blueprint",0,1,0,0,0,0],
    ["Enchanting Faefish","fish_water_fae.blueprint",0,1,0,0,0,0],
    ["School of Fish","fish_water_school.blueprint",0,1,0,0,0,0],
    ["Ancient Seafish","fish_water_ancient.blueprint",0,2,0,0,0,["avoidRod"]],
    ["Wranglerfish","2022/items/item_fish_water_uncommon_anglerfish.blueprint",0,1,1,1,0,0],
    ["Froggy","2022/items/item_fish_water_common_frog.blueprint",0,0,1,1,0,0],
    ["Blue Squarid","2022/items/item_fish_water_uncommon_squid.blueprint",0,1,1,2,0,0],
    ["Sluggo","2022/items/item_fish_water_common_slug.blueprint",0,0,1,2,0,0],
    ["Water Belemental","2022/items/item_fish_water_uncommon_elemental.blueprint",0,1,1,3,0,0],
    ["Pink Cubal","2022/items/item_fish_water_common_coral.blueprint",0,0,1,3,0,0],
    ["Briny Bruce","2022/items/item_fish_water_rare_shark.blueprint",0,2,1,0,0,["poolAny"]],
    ["Gloamfish","fish_undead_ghostfish.blueprint",0,2,2,0,0,["biome", "Cursed Vale"]],
    ["Wide-eyed Noobfish","fish_noobfish.blueprint",0,2,2,0,0,["at", "the tutorial world"]],
    ["Weird Fisheye","fish_eyefish.blueprint",0,2,2,0,0,["at", "the Shadow Tower hub"]],
    ["Hub Hugger","fish_hubhugger.blueprint",0,2,2,0,0,["at", "the hub"]],
    ["Dry Bones","fish_bonefish.blueprint",0,2,2,0,0,["biome", "Desert Frontier"]],
    ["Radiant Moonfish","fish_moonfish.blueprint",0,2,2,0,0,["atNight", "Radiant Ruins"]],
    ["Radiant Dawnfish","fish_sunfish.blueprint",0,2,2,0,0,["atDay", "Radiant Ruins"]],
    ["Rainbow Fish","2022/items/item_fish_water_rare_zebrafish.blueprint",0,2,2,0,0,["biomeIn", "Fae", "Drowned Worlds"]],
    ["Irradium Orefish","2022/items/item_fish_water_uncommon_ore_gl_upper.blueprint",0,1,2,0,0,["biomePart", "upper", "Sundered"]],
    ["Deep Sea Merthing","fish_magic_merqubesly.blueprint",0,2,2,0,1,["belowExcept", 20, "Fae Forest and Dragonfire Peaks"]],
    ["Phoenix Fish","fish_magic_phoenix.blueprint",0,2,2,0,1,["at", "Dragonfire Peaks"]],
    ["Witchly Anemone","fish_magic_witchfunnel.blueprint",0,2,2,0,1,["at", "Cursed Vale"]],
    ["Gryphish","fish_magic_gryphon.blueprint",0,2,2,0,1,["above", 200]],
    ["Frog Prince","fish_magic_frogprince.blueprint",0,2,2,0,1,["at", "Fae Forest"]],
    ["Snugglebug","2022/items/item_fish_enchanted_rare_tardigrade.blueprint",0,2,2,0,2,["poolAt", "the Sea of Eternity, Shores of the Everdark"]],
    ["Neesa","2022/items/item_fish_enchanted_rare_goblinfish.blueprint",0,2,2,0,2,["poolGroup", 3]],
    ["Balefire Frenzyfang","fish_lava_uncommon.blueprint",1,0,0,0,0,0],
    ["Igneous Isopod","fish_lava_epic.blueprint",1,0,0,0,0,0],
    ["Dancing Dragonfish","fish_lava_relic.blueprint",1,0,0,0,0,0],
    ["Shadowspawned Trilobiter","fish_lava_shadow.blueprint",1,0,0,0,0,0],
    ["Fiery Finflapper","fish_lava_rare.blueprint",1,0,0,0,0,0],
    ["Conflagrating Clam","fish_lava_legendary.blueprint",1,0,0,0,0,0],
    ["Rainbow-Shelled Turtleling","fish_lava_resplendent.blueprint",1,0,0,0,0,0],
    ["Shapestone Orefish","fish_orefish_shapestone.blueprint",1,1,0,0,0,0],
    ["Formicite Orefish","fish_orefish_formicite.blueprint",1,1,0,0,0,0],
    ["Infinium Orefish","fish_orefish_infinium.blueprint",1,1,0,0,0,0],
    ["Flamesnout Orefish","fish_lava_fireore.blueprint",1,1,0,0,0,0],
    ["Lava Lancefish","fish_lava_swordfish.blueprint",1,1,0,0,0,0],
    ["Pressurized Coalfish","fish_lava_diamond.blueprint",1,1,0,0,0,0],
    ["Glass Gazer","fish_lava_glass.blueprint",1,1,0,0,0,0],
    ["Ancient Lavarider","fish_lava_ancient.blueprint",1,2,0,0,0,["avoidRod"]],
    ["Molten Wranglerfish","2022/items/item_fish_lava_uncommon_anglerfish.blueprint",1,1,1,1,0,0],
    ["Red Fred","2022/items/item_fish_lava_common_frog.blueprint",1,0,1,1,0,0],
    ["Fire Squarid","2022/items/item_fish_lava_uncommon_squid.blueprint",1,1,1,2,0,0],
    ["Lava Crawler","2022/items/item_fish_lava_common_slug.blueprint",1,0,1,2,0,0],
    ["Fire Belemental","2022/items/item_fish_lava_uncommon_elemental.blueprint",1,1,1,3,0,0],
    ["Fiery Cubal","2022/items/item_fish_lava_common_coral.blueprint",1,0,1,3,0,0],
    ["Ginger","2022/items/item_fish_lava_rare_shark.blueprint",1,2,1,0,0,["poolAny"]],
    ["Tropical Volcanofish","fish_lava_islefireore.blueprint",1,2,2,0,0,["biome", "The Lost Isles"]],
    ["Flameroasted Noobfish","fish_lava_noobfish.blueprint",1,2,2,0,0,["at", "the tutorial world"]],
    ["Frigid Firefish","fish_lava_icefireore.blueprint",1,2,2,0,0,["at", "Permafrost"]],
    ["Charred Hub Hugger","fish_lava_hubhugger.blueprint",1,2,2,0,0,["at", "the hub"]],
    ["Fiery Flow Fish","2022/items/item_fish_lava_rare_zebrafish.blueprint",1,2,2,0,0,["biome", "Jurassic Jungle"]],
    ["Soaring Flamefish","fish_lava_shardine.blueprint",1,2,2,0,0,["at", "the Sky Realm"]],
    ["Emberslag Orefish","2022/items/item_fish_lava_uncommon_ore_gl_lower.blueprint",1,1,2,0,0,["biomePart", "lower", "Sundered"]],
    ["Brightray","2022/items/item_fish_enchanted_rare_ocean_sunfish.blueprint",1,2,2,0,2,["poolAny"]],
    ["Captain Blue Claw","2022/items/item_fish_enchanted_rare_lobster.blueprint",1,2,2,0,2,["poolRare"]],
    ["Mint Choctacoise","fish_choc_uncommon.blueprint",2,0,0,0,0,0],
    ["Blueberry Pie-ranha","fish_choc_rare.blueprint",2,0,0,0,0,0],
    ["Neanderthal Plum Pike","fish_choc_epic.blueprint",2,0,0,0,0,0],
    ["Orange Marlingue","fish_choc_legendary.blueprint",2,0,0,0,0,0],
    ["Cherry Jellyfish","fish_choc_relic.blueprint",2,0,0,0,0,0],
    ["Sour Skate","fish_choc_resplendent.blueprint",2,0,0,0,0,0],
    ["Reef Liquoral","fish_choc_shadow.blueprint",2,0,0,0,0,0],
    ["Crawling Cupcake","fish_choc_cupcake.blueprint",2,1,0,0,0,0],
    ["Fudgsicle Fish","fish_choc_frozenfudge.blueprint",2,1,0,0,0,0],
    ["Candycap Mushfish","fish_water_mushroom.blueprint",2,1,0,0,0,0],
    ["Candied Cutterfish","fish_choc_swordfish.blueprint",2,1,0,0,0,0],
    ["Rich Browniemone","fish_choc_browniemone.blueprint",2,1,0,0,0,0],
    ["Ancient Chocolurker","fish_choc_ancient.blueprint",2,2,0,0,0,["exceptBiomes", "Candoria, or the biome of another rare"]],
    ["Rock Candy Wranglerfish","2022/items/item_fish_chocolate_uncommon_anglerfish.blueprint",2,1,1,1,0,0],
    ["Lollihops","2022/items/item_fish_chocolate_common_frog.blueprint",2,0,1,1,0,0],
    ["Sugared Squarid","2022/items/item_fish_chocolate_uncommon_squid.blueprint",2,1,1,2,0,0],
    ["Candied Crawler","2022/items/item_fish_chocolate_common_slug.blueprint",2,0,1,2,0,0],
    ["Chocolate Belemental","2022/items/item_fish_chocolate_uncommon_elemental.blueprint",2,1,1,3,0,0],
    ["Candied Cubal","2022/items/item_fish_chocolate_common_coral.blueprint",2,0,1,3,0,0],
    ["Brownie","2022/items/item_fish_chocolate_rare_shark.blueprint",2,2,1,0,0,["poolAny"]],
    ["Blue High Flying Cotton Candish","fish_choc_cotcandy_blue.blueprint",2,2,2,0,0,["aboveSplit", 200]],
    ["Pink High Flying Cotton Candish","fish_choc_cotcandy_pink.blueprint",2,2,2,0,0,["aboveSplit", 200]],
    ["Gummy Fish","2022/items/item_fish_chocolate_rare_zebrafish.blueprint",2,2,2,0,0,["biome", "Forbidden Spires"]],
    ["Pressurized Gobfish","fish_choc_gobstopper.blueprint",2,2,2,0,0,["belowExcept", 20, "Candoria"]],
    ["Chocodile","fish_choc_crocodile.blueprint",2,2,2,0,0,["biome", "Candoria"]],
    ["Popular Poptopus","fish_choc_poptopus.blueprint",2,2,2,0,0,["at", "Jurassic Jungle"]],
    ["Cinnabar Orefish","2022/items/item_fish_chocolate_uncommon_ore_cinnabar.blueprint",2,1,2,0,0,["at", "Forbidden Spires"]],
    ["Seasalt Biscuit","2022/items/item_fish_enchanted_rare_sea_urchin.blueprint",2,2,2,0,2,["poolGroup", 1]],
    ["Jade Neon Darter","fish_plasma_uncommon.blueprint",3,0,0,0,0,0],
    ["Ultraviolet Neon Ray","fish_plasma_epic.blueprint",3,0,0,0,0,0],
    ["Carmintine Crab","fish_plasma_relic.blueprint",3,0,0,0,0,0],
    ["Shadow Angler","fish_plasma_shadow.blueprint",3,0,0,0,0,0],
    ["Coldsteel Exofish","fish_plasma_rare.blueprint",3,0,0,0,0,0],
    ["Bronze Neon Drumfish","fish_plasma_legendary.blueprint",3,0,0,0,0,0],
    ["Paragon Prismopod","fish_plasma_resplendent.blueprint",3,0,0,0,0,0],
    ["Closed Betafish","fish_plasma_uncommon_01.blueprint",3,1,0,0,0,0],
    ["Sophisticated Catphish","fish_plasma_uncommon_02.blueprint",3,1,0,0,0,0],
    ["Bug-Infested Alphafish","fish_plasma_uncommon_03.blueprint",3,1,0,0,0,0],
    ["Terabyte Turtle","fish_plasma_uncommon_04.blueprint",3,1,0,0,0,0],
    ["Neon Infinewtie","fish_plasma_uncommon_05.blueprint",3,1,0,0,0,0],
    ["LED Wranglerfish","2022/items/item_fish_plasma_uncommon_anglerfish.blueprint",3,1,1,1,0,0],
    ["B'leep","2022/items/item_fish_plasma_common_frog.blueprint",3,0,1,1,0,0],
    ["Firefly Squarid","2022/items/item_fish_plasma_uncommon_squid.blueprint",3,1,1,2,0,0],
    ["Glowing Sluggo","2022/items/item_fish_plasma_common_slug.blueprint",3,0,1,2,0,0],
    ["Plasma Belemental","2022/items/item_fish_plasma_uncommon_elemental.blueprint",3,1,1,3,0,0],
    ["Bioluminescent Cubal","2022/items/item_fish_plasma_common_coral.blueprint",3,0,1,3,0,0],
    ["Shock Shark Mk. II","2022/items/item_fish_plasma_rare_shark.blueprint",3,2,1,0,0,["poolAny"]],
    ["LED-Lit Lionfish","fish_plasma_rare_01.blueprint",3,2,2,0,0,["biome", "Forbidden Spires"]],
    ["Petrified Pufferfish","fish_plasma_rare_03.blueprint",3,2,2,0,0,["above", 111]],
    ["Zapparapa Eel","fish_plasma_rare_02.blueprint",3,2,2,0,0,["below", 55]],
    ["Protonic Piranhite","fish_plasma_rare_04.blueprint",3,2,2,0,0,["between", "Neon City", 55, 111]],
    ["Octo-BUS Drone","fish_plasma_rare_05.blueprint",3,2,2,0,0,["at", "the Shadow Tower hub"]],
    ["Cooling Plasma Fish","2022/items/item_fish_plasma_rare_zebrafish.blueprint",3,2,2,0,0,["at", "Permafrost"]],
    ["Neon Knightfish","fish_plasma_swordfish.blueprint",3,2,2,0,0,["exceptBiomes", "the Shadow Tower hub, Neon City, Permafrost and Forbidden Spires"]],
    ["Nitro-Glitterine Orefish","2022/items/item_fish_plasma_uncommon_ore_nitro_glitterine.blueprint",3,1,2,0,0,["at", "Geode Topside"]],
    ["Eelvis","2022/items/item_fish_enchanted_rare_eel.blueprint",3,2,2,0,2,["poolGroup", 2]],
    ["Leodo","2022/items/item_fish_enchanted_rare_plasma_turtle.blueprint",3,2,3,0,3,["at", "Geode Topside"]],
    ["Rochelle","2022/items/item_fish_enchanted_rare_lava_turtle.blueprint",1,2,3,0,3,["biomePart", "lower", "Sundered"]],
    ["Michelada","2022/items/item_fish_enchanted_rare_chocolate_turtle.blueprint",2,2,3,0,3,["at", "Permafrost"]],
    ["Dannet","2022/items/item_fish_enchanted_rare_water_turtle.blueprint",0,2,3,0,3,["at", "the Shores of the Everdark"]],
    ["Abyssal Angler","2024/items/item_fish_water_common_maxuber_abyssalangler.blueprint",0,0,4,0,0,0],
    ["Abyssal Crustacean","2024/items/item_fish_water_common_maxuber_abyssalcrustacean.blueprint",0,0,4,0,0,0],
    ["Abyssal Seahorse","2024/items/item_fish_water_uncommon_maxuber_abyssalhippocampus.blueprint",0,1,4,0,0,0],
    ["Abyssal Squid","2024/items/item_fish_water_rare_maxuber_abyssalsquid.blueprint",0,2,4,0,0,0],
    ["Algebrout","2026/items/item_fish_plasma_uncommon_school_trout.blueprint",3,1,4,0,0,0],
    ["Bookmarklin","2026/items/item_fish_plasma_common_school_marlin.blueprint",3,0,4,0,0,0],
    ["Deepstone Fish","2024/items/item_fish_water_uncommon_maxuber_deepstone.blueprint",0,1,4,0,0,0],
    ["Detentuna","2026/items/item_fish_lava_common_school_tuna.blueprint",1,0,4,0,0,0],
    ["Flying Pyric Darter","2024/items/item_fish_water_common_maxuber_pyricflyfish.blueprint",0,0,4,0,0,0],
    ["Krakenspawn","2024/items/item_fish_water_rare_maxuber_kraken.blueprint",0,2,4,0,0,0],
    ["Mediaeveel","2026/items/item_fish_water_common_school_eel.blueprint",0,0,4,0,0,0],
    ["Metronomackerel","2026/items/item_fish_chocolate_uncommon_school_mackerel.blueprint",2,1,4,0,0,0],
    ["Peer Pressurefish","2026/items/item_fish_water_uncommon_school_deepwater.blueprint",0,1,4,0,0,0],
    ["Periodicarp","2026/items/item_fish_lava_uncommon_school_carp.blueprint",1,1,4,0,0,0],
    ["Porked Up Pufferfish","2023/items/item_fish_chocolate_darkwater.blueprint",2,2,4,0,0,0],
    ["Pulsing Reactor Fish","2023/items/item_fish_plasma_darkwater.blueprint",3,2,4,0,0,0],
    ["Pyric Blowfish","2024/items/item_fish_water_uncommon_maxuber_pyricpuffer.blueprint",0,1,4,0,0,0],
    ["Pyric Jellyfish","2024/items/item_fish_water_rare_maxuber_pyricjellyfish.blueprint",0,2,4,0,0,0],
    ["Pyric Krakenspawn","2024/items/item_fish_water_common_maxuber_pyrickraken.blueprint",0,0,4,0,0,0],
    ["Runeslate Fish","2024/items/item_fish_water_uncommon_maxuber_runeslate.blueprint",0,1,4,0,0,0],
    ["Scorched Tigerfish","2023/items/item_fish_lava_darkwater.blueprint",1,2,4,0,0,0],
    ["Smudgeon","2026/items/item_fish_chocolate_common_school_surgeon.blueprint",2,0,4,0,0,0],
    ["Toxic Lichestone Fish","2024/items/item_fish_water_uncommon_maxuber_lichenstone.blueprint",0,1,4,0,0,0],
    ["Zephyr Angler","2024/items/item_fish_water_common_maxuber_zephyrangler.blueprint",0,0,4,0,0,0],
    ["Zephyr Manta","2024/items/item_fish_water_rare_maxuber_zephyrmanta.blueprint",0,2,4,0,0,0],
    ["Zephyr Muscle","2024/items/item_fish_water_uncommon_maxuber_zephyrclam.blueprint",0,1,4,0,0,0],
    ["Zephyr Nautiloid","2024/items/item_fish_water_common_maxuber_zephyrnautiloid.blueprint",0,0,4,0,0,0],
  ];
  var F_NAME = 0, F_BP = 1, F_LIQ = 2, F_RAR = 3, F_SRC = 4, F_GROUP = 5, F_ROD = 6, F_COND = 7;

  function fishOf(test) { return FISH.filter(test); }
  function byRarity(a, b) { return a[F_RAR] - b[F_RAR] || a[F_NAME].localeCompare(b[F_NAME]); }

  /* ── catch conditions ───────────────────────────────────────────────────
     Stored as [template, ...args] so the prose translates while the place
     names - proper nouns in every language - stay as they are. */
  function condText(c) {
    if (!c) return "";
    switch (c[0]) {
      case "biome": return ttf("In the {place} biome", { place: c[1] });
      case "at": return ttf("In {place}", { place: c[1] });
      case "atNight": return ttf("In a {place} world, at night", { place: c[1] });
      case "atDay": return ttf("In a {place} world, by day", { place: c[1] });
      case "biomeIn": return ttf("In a {place} biome in the {world}", { place: c[1], world: c[2] });
      case "biomePart": return c[1] === "upper"
        ? ttf("In the upper part of the {place} biome", { place: c[2] })
        : ttf("In the lower part of the {place} biome", { place: c[2] });
      case "above": return ttf("Anywhere above {n} blocks", { n: c[1] });
      case "aboveSplit": return ttf("Above {n} blocks - a coin flip which colour you get", { n: c[1] });
      case "below": return ttf("Anywhere below {n} blocks", { n: c[1] });
      case "belowExcept": return ttf("Below {n} blocks, outside {place}", { n: c[1], place: c[2] });
      case "between": return ttf("In {place}, between {a} and {b} blocks up", { place: c[1], a: c[2], b: c[3] });
      case "exceptBiomes": return ttf("Anywhere except {place}", { place: c[1] });
      case "avoidRod": return tt("Anywhere - but take off a Lady of the Lake or Royal Reeler first");
      case "poolAny": return tt("Any pool in this liquid");
      case "poolRare": return tt("Rare pools only");
      case "poolGroup": return ttf("Group {n} pools only", { n: c[1] });
      case "poolAt": return ttf("Pools in {place}", { place: c[1] });
      default: return "";
    }
  }

  /* ── fish card ──────────────────────────────────────────────────────────
     The thumbnail is the item's own blueprint, rendered server-side and
     cached; `loading=lazy` keeps 155 of them off the critical path and the
     error handler drops the <img>, so a render miss reads as a plain card
     rather than a broken image. */
  var apiUrl = (window.BTTUtil && window.BTTUtil.apiUrl) || function (p) { return p; };
  function fishIcon(f) {
    var wrap = el("span", { class: "fg-fish-ic" });
    var img = el("img", { loading: "lazy", alt: "",
                          src: apiUrl("/site/codexes/render?blueprint=" + encodeURIComponent(f[F_BP]) + "&dim=96") });
    img.addEventListener("error", function () {
      wrap.classList.add("fg-fish-ic-none");
      if (img.parentNode) img.parentNode.removeChild(img);
    });
    wrap.appendChild(img);
    return wrap;
  }
  function rodChip(rodId) {
    var r = RODS[rodId];
    if (!r) return null;
    return el("span", { class: "fg-rod-chip" }, [el("i", { class: "fa-solid fa-fish-fins" }), " " + r.short]);
  }
  function liqChip(liq) {
    var lq = LIQUIDS[liq];
    if (!lq) return null;
    return el("span", { class: "fg-liq-chip", "data-liq": liq }, [el("i", { class: "fa-solid " + lq.icon }), " " + tt(lq.name)]);
  }
  // What to print when a fish carries no condition of its own. Inside a liquid
  // section the subheading above the grid already says it; in the flat browse
  // table there is no such heading, so each card states its own source.
  function sourceText(f) {
    if (f[F_SRC] === SRC.OPEN) return tt("Out in the open");
    if (f[F_SRC] === SRC.POOL) return f[F_GROUP] ? ttf("Pool group {n}", { n: f[F_GROUP] }) : tt("Any pool in this liquid");
    if (f[F_SRC] === SRC.EXTRA) return tt("Condition not documented here");
    return "";
  }
  function fishCard(f, opts) {
    opts = opts || {};
    var cond = condText(f[F_COND]);
    var meta = el("span", { class: "fg-fish-cond" });
    if (cond) meta.appendChild(doc.createTextNode(cond));
    else if (opts.source) meta.appendChild(doc.createTextNode(sourceText(f)));
    var chip = rodChip(f[F_ROD]);
    if (chip) meta.appendChild(chip);
    if (opts.liquid) meta.appendChild(liqChip(f[F_LIQ]));
    return el("li", { class: "fg-fish", "data-rar": f[F_RAR] }, [
      fishIcon(f),
      el("span", { class: "fg-fish-body" }, [
        el("span", { class: "fg-fish-name", text: f[F_NAME] }),
        meta
      ]),
      el("span", { class: "fg-fish-m", title: tt("Mastery") }, String(RARITY[f[F_RAR]].mastery))
    ]);
  }
  function fishList(rows, opts) {
    var ul = el("ul", { class: "fg-fish-grid" });
    rows.forEach(function (f) { ul.appendChild(fishCard(f, opts)); });
    return ul;
  }
  function subhead(title, note) {
    return el("div", { class: "fg-subhead" }, [
      el("h3", { text: title }),
      note ? el("p", { text: note }) : null
    ]);
  }
  function rarityLegend() {
    var wrap = el("div", { class: "fg-rar-legend" });
    RARITY.forEach(function (r, i) {
      wrap.appendChild(el("span", { class: "fg-rar-key", "data-rar": i }, [
        el("i", { class: "fg-rar-dot" }), tt(r.name) + " · " + r.mastery
      ]));
    });
    return wrap;
  }

  /* ── hero: a fish in the middle of the ripples ──────────────────────── */
  (function () {
    var art = $("#fg-hero-art");
    if (!art) return;
    var ancient = FISH.filter(function (f) { return f[F_NAME] === "Ancient Seafish"; })[0];
    if (!ancient) return;
    var img = el("img", { class: "fg-hero-fish", alt: "",
                          src: apiUrl("/site/codexes/render?blueprint=" + encodeURIComponent(ancient[F_BP]) + "&dim=256") });
    img.addEventListener("error", function () { if (img.parentNode) img.parentNode.removeChild(img); });
    art.appendChild(img);
  })();

  /* ═══════════════════════════════════════════════════════════════════
     1. Special rods
     ═══════════════════════════════════════════════════════════════════ */
  (function () {
    var host = $("#fg-rod-cards");
    if (!host) return;
    var CARDS = [
      { name: "Lady of the Lake", tone: "good", icon: "fa-hat-wizard", rod: 1,
        body: tr("Five water fish exist only on this rod - the Phoenix Fish, Gryphish, Frog Prince, Witchly Anemone and Deep Sea Merthing. Nothing else lands them.") },
      { name: "Murkwater Mark's Mucker", tone: "good", icon: "fa-worm", rod: 2,
        body: tr("The pool rod. A handful of fish - one or two per liquid - bite only in a pool while you're holding it, and some only in one pool group.") },
      { name: "Turtle Trawler", tone: "good", icon: "fa-fish-fins", rod: 3,
        body: tr("The only rod that lands turtles, unlocked on the blue line of the skill tree. Use it outside pools.") },
      { name: "Royal Reeler", tone: "warn", icon: "fa-ban",
        body: tr("This one and the Lady of the Lake skew your catches away from the plain “Ancient” rares. Swap to an ordinary rod when those are what you're after.") }
    ];
    function render() {
      clear(host);
      CARDS.forEach(function (c) {
        var n = c.rod ? fishOf(function (f) { return f[F_ROD] === c.rod; }).length : 0;
        host.appendChild(el("div", { class: "fg-rod-card fg-rod-" + c.tone }, [
          el("i", { class: "fa-solid " + c.icon, "aria-hidden": "true" }),
          el("h4", { text: c.name }),
          el("p", { text: tt(c.body) }),
          c.rod ? el("span", { class: "fg-rod-count", text: ttf("{n} fish need it", { n: n }) }) : null
        ]));
      });
    }
    render(); onLang(render);
  })();

  /* ═══════════════════════════════════════════════════════════════════
     2. Lures - kind x grade
     ═══════════════════════════════════════════════════════════════════ */
  (function () {
    var types = $("#fg-lure-types"), grades = $("#fg-lure-grades"), detail = $("#fg-lure-detail");
    if (!types || !grades || !detail) return;

    var TYPES = [
      { id: 0, name: tr("Quick Lure"), icon: "fa-gauge-high", color: "#ff8a3d",
        effect: tr("Bites come in faster, so the same session gets you through more casts.") },
      { id: 1, name: tr("Depth Lure"), icon: "fa-ruler-vertical", color: "#a371f7",
        effect: tr("More of what you land comes up at trophy size.") },
      { id: 2, name: tr("Seeker Lure"), icon: "fa-magnifying-glass", color: "#f778ba",
        effect: tr("Better odds of an uncommon or a rare instead of a common.") }
    ];
    var GRADES = [
      { id: 0, name: tr("Blue"), color: "#58a6ff", from: tr("Fishing skill tree"), note: tr("The plain version, and perfectly usable.") },
      { id: 1, name: tr("Green"), color: "#3fb950", from: tr("Fishing skill tree"), note: tr("A step up from blue, from further along the tree.") },
      { id: 2, name: tr("Yellow"), color: "#ffd166", from: tr("The turtle merchant"), note: tr("The best grade, sold by the NPC the blue tree line unlocks.") }
    ];
    var t = 2, g = 2;

    function build() {
      clear(types); clear(grades);
      TYPES.forEach(function (x) {
        var b = el("button", { class: "fg-lure-type" + (x.id === t ? " active" : ""), type: "button",
                               role: "radio", "aria-checked": x.id === t ? "true" : "false" }, [
          el("i", { class: "fa-solid " + x.icon, "aria-hidden": "true" }),
          el("span", { text: tt(x.name) })
        ]);
        b.style.setProperty("--lc", x.color);
        b.addEventListener("click", function () { t = x.id; build(); });
        types.appendChild(b);
      });
      GRADES.forEach(function (x) {
        var b = el("button", { class: "fg-lure-grade" + (x.id === g ? " active" : ""), type: "button",
                               role: "radio", "aria-checked": x.id === g ? "true" : "false", text: tt(x.name) });
        b.style.setProperty("--lc", x.color);
        b.addEventListener("click", function () { g = x.id; build(); });
        grades.appendChild(b);
      });
      render();
    }
    function render() {
      var T = TYPES[t], G = GRADES[g];
      clear(detail);
      detail.style.setProperty("--lc", T.color);
      detail.style.setProperty("--gc", G.color);
      detail.appendChild(el("div", { class: "fg-lure-head" }, [
        el("span", { class: "fg-lure-swatch", "aria-hidden": "true" }),
        el("h3", { text: ttf("{grade} {type}", { grade: tt(G.name), type: tt(T.name) }) })
      ]));
      detail.appendChild(el("p", { class: "fg-lure-effect", text: tt(T.effect) }));
      detail.appendChild(el("dl", { class: "fg-lure-facts" }, [
        el("div", {}, [el("dt", { text: tt("Grade") }), el("dd", { text: tt(G.note) })]),
        el("div", {}, [el("dt", { text: tt("Comes from") }), el("dd", { text: tt(G.from) })])
      ]));
    }
    build(); onLang(build);
  })();

  /* ═══════════════════════════════════════════════════════════════════
     3. Skill tree - a schematic, not a map. Four branches out of a hub with
     the reward nodes marked; the legend cross-highlights them.
     ═══════════════════════════════════════════════════════════════════ */
  (function () {
    var svg = $("#fg-tree-svg"), legend = $("#fg-tree-legend");
    if (!svg || !legend) return;
    var NS = "http://www.w3.org/2000/svg";

    var KINDS = [
      { key: "blue", color: "#58a6ff", label: tr("The blue line"),
        body: tr("Unlocks the Turtle Trawler rod and the merchant that sells the yellow lures.") },
      { key: "pink", color: "#f778ba", label: tr("Three pink nodes"),
        body: tr("Caprian dragon fragments, one per node, spread across the branches.") },
      { key: "orange", color: "#ff8a3d", label: tr("Three orange nodes"),
        body: tr("An ally, one node at a time.") },
      { key: "white", color: "#eaf1fa", label: tr("One white node"),
        body: tr("Raises the daily quest cap from three to six.") }
    ];
    // Branch = a chain of points out of the hub; `reward` marks which of them
    // is one of the four node types worth chasing.
    var HUB = [160, 160];
    var BRANCHES = [
      { blue: true, pts: [[160, 122], [186, 96], [214, 78], [244, 90], [254, 122]],
        reward: [{ i: 1, k: "blue", big: true }, { i: 5, k: "blue", big: false }] },
      { pts: [[122, 160], [92, 140], [62, 122], [40, 140]],
        reward: [{ i: 2, k: "pink", big: false }, { i: 4, k: "orange", big: true }] },
      { pts: [[198, 160], [228, 146], [258, 132], [282, 150]],
        reward: [{ i: 2, k: "pink", big: false }, { i: 4, k: "orange", big: true }] },
      { pts: [[160, 198], [148, 228], [124, 252], [92, 264], [62, 250]],
        reward: [{ i: 3, k: "pink", big: false }, { i: 4, k: "orange", big: true }, { i: 5, k: "white", big: false }] }
    ];

    function mk(tag, attrs) {
      var n = doc.createElementNS(NS, tag), k;
      for (k in attrs) n.setAttribute(k, attrs[k]);
      return n;
    }
    function build() {
      clear(svg);
      svg.appendChild(mk("circle", { cx: 160, cy: 160, r: 150, class: "fg-tree-rim" }));
      BRANCHES.forEach(function (br) {
        var pts = [HUB].concat(br.pts), rewardAt = {}, i;
        br.reward.forEach(function (r) { rewardAt[r.i] = r; });
        for (i = 1; i < pts.length; i++) {
          svg.appendChild(mk("line", {
            x1: pts[i - 1][0], y1: pts[i - 1][1], x2: pts[i][0], y2: pts[i][1],
            class: "fg-tree-link" + (br.blue ? " fg-tree-link-blue" : "")
          }));
        }
        pts.forEach(function (p, idx) {
          if (!idx) return;
          var r = rewardAt[idx];
          var node = mk("circle", { cx: p[0], cy: p[1], r: r ? (r.big ? 8 : 6.5) : 4.2,
                                    class: "fg-tree-node" + (r ? " fg-tree-node-reward" : "") });
          if (r) node.setAttribute("data-kind", r.k);
          svg.appendChild(node);
        });
      });
      svg.appendChild(mk("circle", { cx: 160, cy: 160, r: 7.5, class: "fg-tree-hub" }));

      clear(legend);
      KINDS.forEach(function (k) {
        var li = el("li", { class: "fg-tree-item", "data-kind": k.key }, [
          el("span", { class: "fg-tree-dot", "aria-hidden": "true" }),
          el("div", {}, [el("b", { text: tt(k.label) }), el("span", { text: tt(k.body) })])
        ]);
        li.style.setProperty("--kc", k.color);
        li.tabIndex = 0;
        function on() { svg.setAttribute("data-focus", k.key); }
        function off() { svg.removeAttribute("data-focus"); }
        li.addEventListener("mouseenter", on);
        li.addEventListener("mouseleave", off);
        li.addEventListener("focus", on);
        li.addEventListener("blur", off);
        legend.appendChild(li);
      });
      legend.appendChild(el("li", { class: "fg-tree-item fg-tree-item-plain" }, [
        el("span", { class: "fg-tree-dot fg-tree-dot-plain", "aria-hidden": "true" }),
        el("div", {}, [
          el("b", { text: tt("Everything else") }),
          el("span", { text: tt("Small bonuses - including the node that gives an emptied pool a chance to refill.") })
        ])
      ]));
    }
    build(); onLang(build);
  })();

  /* ═══════════════════════════════════════════════════════════════════
     4. Daily quests
     ═══════════════════════════════════════════════════════════════════ */
  (function () {
    var randHost = $("#fg-quest-random"), specHost = $("#fg-quest-specific");
    var totalOut = $("#fg-quest-total"), keyOut = $("#fg-quest-key"), hint = $("#fg-quest-hint"), node = $("#fg-quest-node");
    if (!randHost || !specHost || !totalOut || !node) return;

    var TIERS = [
      { name: tr("Bronze"), color: "#d08c5a", ask: tr("50 commons"), marks: 175 },
      { name: tr("Silver"), color: "#c6cfdb", ask: tr("15 uncommons"), marks: 225 },
      { name: tr("Gold"), color: "#ffd166", ask: tr("2 rares"), marks: 275 }
    ];
    var picks = [0, 1, 2, 0, 1, 2];   // slots 0-2 random, 3-5 liquid-specific
    var slots = [];

    function slot(i, host, liquidId) {
      var b = el("button", { class: "fg-quest-slot", type: "button" });
      function paint() {
        var T = TIERS[picks[i]];
        clear(b);
        b.style.setProperty("--qc", T.color);
        b.classList.toggle("locked", i > 2 && !node.checked);
        b.appendChild(el("span", { class: "fg-quest-tier", text: tt(T.name) }));
        b.appendChild(el("span", { class: "fg-quest-ask", text: tt(T.ask) }));
        if (liquidId != null) b.appendChild(liqChip(liquidId));
        b.appendChild(el("span", { class: "fg-quest-marks", text: "+" + T.marks }));
      }
      b.addEventListener("click", function () { picks[i] = (picks[i] + 1) % 3; paint(); total(); });
      b._paint = paint;
      paint();
      host.appendChild(b);
      return b;
    }
    function total() {
      var n = node.checked ? 6 : 3, sum = 0, i;
      for (i = 0; i < n; i++) sum += TIERS[picks[i]].marks;
      totalOut.textContent = sum.toLocaleString();
      hint.textContent = node.checked
        ? ttf("Six a day: three any liquid satisfies, three tied to one named liquid. All gold pays {n} Marks.", { n: (275 * 6).toLocaleString() })
        : tt("Without the skill-tree node you're capped at three quests a day.");
    }
    function build() {
      clear(randHost); clear(specHost); slots = [];
      var i;
      for (i = 0; i < 3; i++) slots.push(slot(i, randHost, null));
      for (i = 3; i < 6; i++) slots.push(slot(i, specHost, i - 3));
      clear(keyOut);
      TIERS.forEach(function (T) {
        var li = el("li", {}, [
          el("b", { text: tt(T.name) }),
          " " + ttf("{ask} → {marks} Marks", { ask: tt(T.ask), marks: T.marks })
        ]);
        li.style.setProperty("--qc", T.color);
        keyOut.appendChild(li);
      });
      total();
    }
    node.addEventListener("change", function () {
      slots.forEach(function (s) { s._paint(); });
      total();
    });
    build(); onLang(build);
  })();

  /* ═══════════════════════════════════════════════════════════════════
     5. Pool simulator - group lock and depletion, the two rules that catch
     people out. Deliberately says nothing about drop rates.
     ═══════════════════════════════════════════════════════════════════ */
  (function () {
    var stage = $("#fg-pool-water"), dots = $("#fg-pool-fish"), countOut = $("#fg-pool-count");
    var castBtn = $("#fg-pool-cast"), resetBtn = $("#fg-pool-reset"), rareBox = $("#fg-pool-israre");
    var status = $("#fg-pool-status"), log = $("#fg-pool-catches");
    if (!stage || !castBtn || !rareBox) return;

    var liquid = 0, group = 0, left = 0, isRare = false, picker = null;
    function poolFish(liq, grp) {
      return fishOf(function (f) { return f[F_SRC] === SRC.POOL && f[F_LIQ] === liq && f[F_GROUP] === grp; });
    }
    function poolRare(liq) {
      return fishOf(function (f) { return f[F_SRC] === SRC.POOL && f[F_LIQ] === liq && !f[F_GROUP]; })[0];
    }

    function reset() {
      isRare = !!rareBox.checked;
      group = 0;
      left = isRare ? 3 + Math.floor(Math.random() * 2) : 6 + Math.floor(Math.random() * 5);
      stage.setAttribute("data-liq", String(liquid));
      stage.classList.toggle("rare", isRare);
      stage.classList.remove("closed");
      clear(log);
      paint();
      status.textContent = isRare
        ? tt("A rare pool: three or four fish and every one of them rare. Fish it out before you do anything else.")
        : tt("A fresh pool. The first fish you land decides which of the three groups this one holds.");
    }
    function paint() {
      countOut.textContent = String(left);
      clear(dots);
      for (var i = 0; i < left; i++) {
        var d = el("span", { class: "fg-pool-dot" });
        d.style.setProperty("--d", String(i));
        dots.appendChild(d);
      }
      castBtn.disabled = left <= 0;
    }
    function cast() {
      if (left <= 0) return;
      var caught;
      if (isRare) {
        caught = poolRare(liquid);
      } else {
        if (!group) group = 1 + Math.floor(Math.random() * 3);
        var options = poolFish(liquid, group);
        caught = options[Math.floor(Math.random() * options.length)];
      }
      left--;
      paint();
      if (caught) {
        log.insertBefore(el("li", { class: "fg-pool-catch", "data-rar": caught[F_RAR] }, [
          fishIcon(caught),
          el("span", { class: "fg-pool-catch-n", text: caught[F_NAME] }),
          el("span", { class: "fg-pool-catch-m", text: "+" + RARITY[caught[F_RAR]].mastery })
        ]), log.firstChild);
      }
      if (left <= 0) {
        stage.classList.add("closed");
        status.textContent = tt("Empty, so the pool closes. One skill-tree node gives it a chance to refill, but that's a roll.");
      } else if (!isRare) {
        status.textContent = ttf("Locked to group {n}. Nothing outside that group bites here now - the other two live in other pools.", { n: group });
      }
    }
    function buildPicker() {
      if (!picker) {
        picker = el("div", { class: "fg-pool-liquids", role: "radiogroup" });
        stage.parentNode.insertBefore(picker, stage);
      }
      picker.setAttribute("aria-label", tt("Pool liquid"));
      clear(picker);
      LIQUIDS.forEach(function (lq, i) {
        var b = el("button", { class: "fg-liq-btn" + (i === liquid ? " active" : ""), type: "button", role: "radio",
                               "aria-checked": i === liquid ? "true" : "false", "data-liq": i }, [
          el("i", { class: "fa-solid " + lq.icon, "aria-hidden": "true" }),
          el("span", { text: tt(lq.name) })
        ]);
        b.addEventListener("click", function () { liquid = i; buildPicker(); reset(); });
        picker.appendChild(b);
      });
    }
    castBtn.addEventListener("click", cast);
    resetBtn.addEventListener("click", reset);
    rareBox.addEventListener("change", reset);
    buildPicker(); reset();
    onLang(function () { buildPicker(); reset(); });
  })();

  /* ═══════════════════════════════════════════════════════════════════
     6. Per-liquid fish tables
     ═══════════════════════════════════════════════════════════════════ */
  (function () {
    var sections = Array.prototype.slice.call(doc.querySelectorAll(".fg-liquid"));
    if (!sections.length) return;

    function build() {
      sections.forEach(function (sec) {
        var liq = +sec.getAttribute("data-liquid");
        var host = clear($(".fg-liquid-body", sec));
        sec.style.setProperty("--liq", LIQUIDS[liq].color);

        host.appendChild(rarityLegend());

        var open = fishOf(function (f) { return f[F_LIQ] === liq && f[F_SRC] === SRC.OPEN; }).sort(byRarity);
        host.appendChild(subhead(tt("Out in the open"),
          tt("Any rod, anywhere in this liquid - as long as there's a lure on it.")));
        host.appendChild(fishList(open));

        host.appendChild(subhead(tt("Inside pools"),
          tt("Pool-only fish. A pool gives you one group and one group only, so read each row as a single pool's whole catch list.")));
        var groups = el("div", { class: "fg-groups" });
        [1, 2, 3].forEach(function (g) {
          var rows = fishOf(function (f) { return f[F_LIQ] === liq && f[F_SRC] === SRC.POOL && f[F_GROUP] === g; }).sort(byRarity);
          if (!rows.length) return;
          groups.appendChild(el("div", { class: "fg-group" }, [
            el("span", { class: "fg-group-tag", text: ttf("Group {n}", { n: g }) }),
            fishList(rows)
          ]));
        });
        var anyGroup = fishOf(function (f) { return f[F_LIQ] === liq && f[F_SRC] === SRC.POOL && !f[F_GROUP]; });
        if (anyGroup.length) {
          groups.appendChild(el("div", { class: "fg-group fg-group-any" }, [
            el("span", { class: "fg-group-tag", text: tt("Any group") }),
            fishList(anyGroup)
          ]));
        }
        host.appendChild(groups);

        var special = fishOf(function (f) { return f[F_LIQ] === liq && f[F_SRC] === SRC.SPECIAL; })
          .sort(function (a, b) { return a[F_ROD] - b[F_ROD] || byRarity(a, b); });
        if (special.length) {
          host.appendChild(subhead(tt("Conditional catches"),
            tt("These want something specific: a biome, a height, a time of day, or a particular rod.")));
          host.appendChild(fishList(special));
        }

        var extra = fishOf(function (f) { return f[F_LIQ] === liq && f[F_SRC] === SRC.EXTRA; }).sort(byRarity);
        if (extra.length) {
          var det = el("details", { class: "fg-extra" });
          det.appendChild(el("summary", {}, [
            el("span", { class: "fg-extra-badge", text: tt("Not documented") }),
            el("span", { text: ttf("{n} more fish the codex lists in this liquid", { n: extra.length }) }),
            el("i", { class: "fa-solid fa-chevron-down fg-extra-chev", "aria-hidden": "true" })
          ]));
          det.appendChild(el("p", { class: "fg-extra-note",
            text: tt("They're in the game and in the codex, but this guide doesn't cover what it takes to catch them.") }));
          det.appendChild(fishList(extra, { source: true }));
          host.appendChild(det);
        }
      });
    }
    build(); onLang(build);
  })();

  /* ═══════════════════════════════════════════════════════════════════
     7. Turtles
     ═══════════════════════════════════════════════════════════════════ */
  (function () {
    var host = $("#fg-turtle-grid");
    if (!host) return;
    function build() {
      clear(host);
      fishOf(function (f) { return f[F_SRC] === SRC.TURTLE; }).forEach(function (f) {
        host.appendChild(el("div", { class: "fg-turtle", "data-liq": f[F_LIQ] }, [
          fishIcon(f),
          el("h3", { text: f[F_NAME] }),
          liqChip(f[F_LIQ]),
          el("p", { text: condText(f[F_COND]) })
        ]));
      });
    }
    build(); onLang(build);
  })();

  /* ═══════════════════════════════════════════════════════════════════
     8. Browse everything
     ═══════════════════════════════════════════════════════════════════ */
  (function () {
    var grid = $("#fg-browse-grid"), search = $("#fg-search"), filters = $("#fg-filters"), countOut = $("#fg-browse-count");
    if (!grid || !search || !filters) return;

    var fLiq = null, fRar = null, q = "";

    function matches(f) {
      if (fLiq != null && f[F_LIQ] !== fLiq) return false;
      if (fRar != null && f[F_RAR] !== fRar) return false;
      if (!q) return true;
      var hay = (f[F_NAME] + " " + condText(f[F_COND]) + " " +
                 (RODS[f[F_ROD]] ? RODS[f[F_ROD]].name : "") + " " + tt(LIQUIDS[f[F_LIQ]].name)).toLowerCase();
      return hay.indexOf(q) >= 0;
    }
    function render() {
      var rows = FISH.filter(matches).sort(function (a, b) {
        return a[F_LIQ] - b[F_LIQ] || a[F_RAR] - b[F_RAR] || a[F_NAME].localeCompare(b[F_NAME]);
      });
      clear(grid);
      grid.appendChild(fishList(rows, { liquid: true, source: true }));
      countOut.textContent = ttf("{n} of {total} fish", { n: rows.length, total: FISH.length });
    }
    function chip(label, on, onclick) {
      var b = el("button", { class: "fg-chip-btn" + (on ? " active" : ""), type: "button",
                             "aria-pressed": on ? "true" : "false", text: label });
      b.addEventListener("click", onclick);
      return b;
    }
    function build() {
      clear(filters);
      filters.appendChild(chip(tt("All liquids"), fLiq == null, function () { fLiq = null; build(); }));
      LIQUIDS.forEach(function (lq, i) {
        var b = chip(tt(lq.name), fLiq === i, function () { fLiq = (fLiq === i ? null : i); build(); });
        b.setAttribute("data-liq", String(i));
        filters.appendChild(b);
      });
      filters.appendChild(el("span", { class: "fg-filter-sep", "aria-hidden": "true" }));
      RARITY.forEach(function (r, i) {
        var b = chip(tt(r.name), fRar === i, function () { fRar = (fRar === i ? null : i); build(); });
        b.setAttribute("data-rar", String(i));
        filters.appendChild(b);
      });
      render();
    }
    search.addEventListener("input", function () { q = search.value.trim().toLowerCase(); render(); });
    build(); onLang(build);

    var chipCount = $("#fg-chip-count");
    if (chipCount) chipCount.textContent = String(FISH.length);
  })();

  /* ═══════════════════════════════════════════════════════════════════
     Reveal-on-scroll + section-nav highlight
     ═══════════════════════════════════════════════════════════════════ */
  (function () {
    body.classList.add("fg-js");
    var reveals = Array.prototype.slice.call(doc.querySelectorAll("[data-reveal]"));
    reveals.forEach(function (n) {
      var sibs = Array.prototype.filter.call(n.parentNode.children, function (c) {
        return c.hasAttribute && c.hasAttribute("data-reveal");
      });
      n.style.setProperty("--i", String(sibs.indexOf(n)));
    });

    if (REDUCE) {
      reveals.forEach(function (n) { n.classList.add("in"); });
    } else {
      // Sweep on load + throttled scroll rather than a pure IntersectionObserver,
      // so a headless render (or a #hash deep link) never ships a blank page.
      var pending = reveals.slice();
      var sweep = function () {
        var h = window.innerHeight || doc.documentElement.clientHeight;
        pending = pending.filter(function (n) {
          var r = n.getBoundingClientRect();
          if (r.top < h * 0.92 && r.bottom > 0) { n.classList.add("in"); return false; }
          return true;
        });
        if (!pending.length) { window.removeEventListener("scroll", sweep); window.removeEventListener("resize", sweep); }
      };
      window.addEventListener("scroll", sweep, { passive: true });
      window.addEventListener("resize", sweep);
      window.addEventListener("load", sweep);
      sweep();
    }

    var secnav = $("#fg-secnav");
    if (secnav && "IntersectionObserver" in window) {
      var links = Array.prototype.slice.call(secnav.querySelectorAll("a"));
      var map = {};
      links.forEach(function (a) { map[a.getAttribute("href").slice(1)] = a; });
      var sections = links.map(function (a) { return doc.getElementById(a.getAttribute("href").slice(1)); }).filter(Boolean);
      var io = new IntersectionObserver(function (entries) {
        entries.forEach(function (e) {
          if (!e.isIntersecting) return;
          links.forEach(function (l) { l.classList.remove("active"); });
          if (map[e.target.id]) map[e.target.id].classList.add("active");
        });
      }, { rootMargin: "-45% 0px -50% 0px", threshold: 0 });
      sections.forEach(function (s) { io.observe(s); });
    }
  })();

  /* ═══════════════════════════════════════════════════════════════════
     Guide / Wiki view toggle - same contract as /gems-guide: Wiki drops the
     narrative leads and shows one section at a time, the secnav acting as
     tabs. ?view= wins over the saved preference so a shared link keeps its
     view.
     ═══════════════════════════════════════════════════════════════════ */
  (function () {
    var secnav = $("#fg-secnav");
    if (!secnav) return;
    var KEY = "fg-view";
    function readView() {
      var qv;
      try { qv = new URLSearchParams(location.search).get("view"); } catch (e) {}
      if (qv === "wiki" || qv === "guide") return qv;
      try { var s = localStorage.getItem(KEY); if (s === "wiki" || s === "guide") return s; } catch (e) {}
      return "guide";
    }
    function writeUrl(v, tab) {
      try {
        var u = new URL(location.href);
        u.searchParams.set("view", v);
        if (v === "wiki" && tab) u.hash = tab;
        history.replaceState(null, "", u.pathname + u.search + u.hash);
      } catch (e) {}
    }
    var view = readView();

    var lbl = el("span", { class: "fg-view-toggle-lbl", text: tt("View") });
    var bGuide = el("button", { type: "button", role: "radio", text: tt("Guide") });
    var bWiki = el("button", { type: "button", role: "radio", text: tt("Wiki") });
    var seg = el("div", { class: "fg-view-seg", role: "radiogroup", "aria-label": tt("Page view") }, [bGuide, bWiki]);
    var bar = el("div", { class: "fg-view-toggle" }, [lbl, seg]);
    secnav.parentNode.insertBefore(bar, secnav);

    var links = Array.prototype.slice.call(secnav.querySelectorAll("a"));
    var sections = links.map(function (a) { return doc.getElementById(a.getAttribute("href").slice(1)); });
    var curTab = null;

    function showTab(id, scroll) {
      curTab = id;
      var activeLink = null;
      sections.forEach(function (s, i) {
        if (!s) return;
        var on = s.id === id;
        s.classList.toggle("fg-wiki-active", on);
        links[i].classList.toggle("active", on);
        if (on) activeLink = links[i];
      });
      // Narrow screens keep the nav as one scrolling row - centre the selected
      // pill so the tab you just picked is never parked off-screen.
      if (activeLink && secnav.scrollWidth > secnav.clientWidth + 1) {
        var lr = activeLink.getBoundingClientRect(), nr = secnav.getBoundingClientRect();
        var left = secnav.scrollLeft + (lr.left + lr.width / 2) - (nr.left + nr.width / 2);
        if (secnav.scrollTo) secnav.scrollTo({ left: left, behavior: REDUCE ? "auto" : "smooth" });
        else secnav.scrollLeft = left;
      }
      if (scroll) { var t = doc.getElementById(id); if (t) t.scrollIntoView({ block: "start" }); }
      if (view === "wiki") writeUrl("wiki", id);
    }
    function firstTab() {
      var h = (location.hash || "").slice(1);
      var ids = sections.filter(Boolean).map(function (s) { return s.id; });
      return ids.indexOf(h) >= 0 ? h : (ids[0] || null);
    }
    function apply(v, scroll) {
      view = v;
      body.classList.toggle("fg-wiki", v === "wiki");
      bGuide.classList.toggle("active", v === "guide");
      bWiki.classList.toggle("active", v === "wiki");
      bGuide.setAttribute("aria-checked", v === "guide" ? "true" : "false");
      bWiki.setAttribute("aria-checked", v === "wiki" ? "true" : "false");
      if (v === "wiki") showTab(curTab || firstTab(), scroll);
      else writeUrl("guide", null);
    }
    function remember(v) { try { localStorage.setItem(KEY, v); } catch (e) {} }

    links.forEach(function (a) {
      a.addEventListener("click", function (ev) {
        if (view !== "wiki") return;      // guide mode: normal anchor scroll
        ev.preventDefault();
        showTab(a.getAttribute("href").slice(1), true);
      });
    });
    bGuide.addEventListener("click", function () { apply("guide", false); remember("guide"); });
    bWiki.addEventListener("click", function () { apply("wiki", true); remember("wiki"); });

    apply(view, false);
    onLang(function () {
      lbl.textContent = tt("View");
      bGuide.textContent = tt("Guide"); bWiki.textContent = tt("Wiki");
      seg.setAttribute("aria-label", tt("Page view"));
    });
  })();

})();

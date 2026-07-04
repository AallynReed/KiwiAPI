// Normalize a parsed .pkfx document into a runtime Effect: a list of particle
// Layers, each with resolved fields, samplers, a compiled spawn script, an ordered
// evolver list, and a flat list of renderers.
import { deref, toNums, toSym } from './parser.js';
import { compileScript } from './script.js';
import { CurveSampler, DoubleCurveSampler, ShapeSampler } from './curves.js';

const FIELD_COMP = { float: 1, float2: 2, float3: 3, float4: 4, int: 1, int2: 2, int3: 3, int4: 4 };
// Built-in fields every layer has, with default component counts.
const BUILTINS = { Life: 1, Age: 1, Position: 3, Velocity: 3, Size: 2, Color: 4, Rotation: 1, TextureID: 1 };

export function buildEffect(doc, rng) {
  const root = findRoot(doc);
  if (!root) throw new Error('no CParticleEffect found');

  // global samplers + attribute defaults exposed to every layer (from CustomAttributes).
  // Attributes are the editor/game-supplied "conditions" (EmissionRate, SizeMult, …);
  // the game sets them at runtime, so for a preview we use their declared defaults.
  const globalSamplers = {};
  const attributes = {};
  const al = deref(doc, root.props.CustomAttributes);
  if (al) {
    for (const ref of al.props.SamplerList || []) addSampler(doc, deref(doc, ref), globalSamplers, rng);
    for (const ref of al.props.AttributeList || []) {
      const a = deref(doc, ref); if (!a || !a.props.AttributeName) continue;
      attributes[a.props.AttributeName] = trimVec(toNums(a.props.DefaultValueF4) || [0, 0, 0, 0]);
    }
  }

  // Build the layer TREE: root layers (OnSpawn) + child layers reached via sub-emitters
  // (trail spawner-evolvers and script events). Layers reference children BY INDEX, so a
  // parent particle can spawn into its child layer at runtime (the real trigger).
  const ctx = { doc, rng, globalSamplers, attributes, layers: [], indexByDesc: new Map() };
  const rootSpawners = [];
  collectSpawners(doc, deref(doc, root.props.OnSpawn), rootSpawners);
  for (const sp of rootSpawners) ensureLayer(ctx, deref(doc, sp.props.Descriptor), sp);

  return { root, layers: ctx.layers, attributes, looping: toSym(deref(doc, root.props.OnSpawn)?.props?.IsLooping) };
}

// Build a layer for a descriptor if not already built; return its index. `spawner` is the
// action that spawns it (null for child layers, which only spawn via a parent). Reserves
// the index BEFORE recursing into children so cycles terminate.
function ensureLayer(ctx, desc, spawner) {
  if (!desc) return -1;
  if (ctx.indexByDesc.has(desc.id)) return ctx.indexByDesc.get(desc.id);
  const idx = ctx.layers.length;
  ctx.indexByDesc.set(desc.id, idx);
  ctx.layers.push(null);                 // placeholder (filled below)
  ctx.layers[idx] = buildLayer(ctx, desc, spawner);
  return idx;
}

// keep at least one component, drop trailing zeros (float4(1,0,0,0)->[1]; (r,g,b,a) stays)
function trimVec(v) {
  let n = v.length;
  while (n > 1 && v[n - 1] === 0) n--;
  return v.slice(0, n);
}

function findRoot(doc) {
  for (const id of doc.order) if (doc.objects[id].className === 'CParticleEffect') return doc.objects[id];
  return null;
}

function collectSpawners(doc, node, out, depth = 0) {
  if (!node || depth > 32) return;
  const cn = node.className;
  if (cn === 'CActionFactoryParticleSpawnerBase') { out.push(node); return; }
  if (cn === 'CActionFactoryWithChilds' || cn === 'CActionFactoryWithRandomChilds') {
    for (const ref of node.props.ChildList || []) collectSpawners(doc, deref(doc, ref), out, depth + 1);
  }
  // other action factories (sound, entity) are ignored for rendering
}

function buildLayer(ctx, desc, spawner) {
  const { doc, rng, globalSamplers } = ctx;
  // ---- fields ----
  const fields = []; const fieldIndex = {};
  const addField = (name, comp) => {
    if (fieldIndex[name]) { if (comp > fieldIndex[name].comp) {/* keep larger */} return; }
    fieldIndex[name] = { offset: 0, comp }; fields.push({ name, comp });
  };
  for (const [name, comp] of Object.entries(BUILTINS)) addField(name, comp);
  for (const ref of desc.props.CustomFields || []) {
    const f = deref(doc, ref); if (!f) continue;
    const comp = FIELD_COMP[toSym(f.props.FieldType)] ?? 1;
    addField(f.props.FieldName, comp);
  }
  // assign offsets (SoA stride)
  let stride = 0; for (const f of fields) { fieldIndex[f.name].offset = stride; stride += f.comp; }

  // ---- samplers (descriptor-local + global) ----
  const samplers = Object.assign({}, globalSamplers);
  for (const ref of desc.props.Samplers || []) addSampler(doc, deref(doc, ref), samplers, rng);

  // ---- spawn script ----
  let spawnScript = null;
  const se = deref(doc, desc.props.SpawnEvaluator);
  if (se && typeof se.props.Expression === 'string') spawnScript = tryCompile(se.props.Expression);

  // ---- evolvers (ordered) — trail spawner-evolvers resolve to a child layer INDEX ----
  const evolvers = [];
  const state = deref(doc, (desc.props.States || [])[0]);
  if (state) for (const ref of state.props.Evolvers || []) addEvolver(ctx, deref(doc, ref), evolvers, samplers);

  // ---- script events: EventName -> [{ child layer index, spawn count }] ----
  const events = {};
  for (const ref of desc.props.CustomEvents || []) {
    const ed = deref(doc, ref); if (!ed || !ed.props.EventName) continue;
    const evSpawners = []; collectSpawners(doc, deref(doc, ed.props.EventAction), evSpawners);
    const targets = [];
    for (const sp of evSpawners) {
      const cd = deref(doc, sp.props.Descriptor); if (!cd) continue;
      targets.push({ layer: ensureLayer(ctx, cd, null), count: Math.max(1, Math.round(num(sp.props.SpawnCount, 1))) });
    }
    if (targets.length) events[ed.props.EventName] = (events[ed.props.EventName] || []).concat(targets);
  }

  // ---- renderers (flattened) ----
  const renderers = [];
  collectRenderers(doc, deref(doc, desc.props.Renderer), renderers);

  // spawn timing (child layers have no spawner — they spawn only from a parent particle)
  const sp = spawner ? spawner.props : {};
  return {
    name: (spawner ? spawner.id : desc.id).replace('$LOCAL$/', ''),
    isChild: !spawner,
    fields, fieldIndex, stride,
    samplers, spawnScript, evolvers, events, renderers,
    spawnCount: num(sp.SpawnCount, 0),
    duration: num(sp.DurationInSeconds, 0),
    infinite: sp.Infinite === true,
    delay: num(sp.Delay, 0),
  };
}

function addSampler(doc, obj, into, rng) {
  if (!obj) return;
  const name = obj.props.SamplerName; if (!name) return;
  into[name] = makeSampler(doc, obj, rng);
}
function makeSampler(doc, obj, rng) {
  switch (obj.className) {
    case 'CParticleSamplerCurve': return new CurveSampler(obj);
    case 'CParticleSamplerDoubleCurve': return new DoubleCurveSampler(obj);
    case 'CParticleSamplerShape': return new ShapeSampler(deref(doc, obj.props.Shape), rng);
    default: return { sample: () => [0] };
  }
}

function addEvolver(ctx, ev, out, samplers) {
  if (!ev) return;
  const { doc, rng } = ctx;
  switch (ev.className) {
    case 'CParticleEvolver_Physics':
      out.push({ type: 'physics', accel: toNums(ev.props.ConstantAcceleration) || [0, 0, 0], drag: num(ev.props.Drag, 0) });
      break;
    case 'CParticleEvolver_Field': {
      const curve = makeSampler(doc, deref(doc, ev.props.Evaluator), rng);
      out.push({ type: 'field', field: ev.props.Name, curve });
      break;
    }
    case 'CParticleEvolver_Script': {
      const expr = deref(doc, ev.props.Expression);
      const sc = expr && typeof expr.props.Expression === 'string' ? tryCompile(expr.props.Expression) : null;
      if (sc) out.push({ type: 'script', script: sc });
      break;
    }
    case 'CParticleEvolver_FlipBook':
      out.push({ type: 'flipbook', lastFrame: num(ev.props.LastFrameID, 0) });
      break;
    case 'CParticleEvolver_Rotation':
      out.push({ type: 'rotation', speed: num(ev.props.ScalarRotationSpeed, 0) });
      break;
    case 'CParticleEvolver_Damper':
      out.push({ type: 'damper', field: ev.props.FieldToDampen, time: num(ev.props.ExpDampingTime, 0.1) });
      break;
    case 'CParticleEvolver_Spawner': {
      // a trail: each parent particle emits into the child layer over time/distance
      const child = ensureLayer(ctx, deref(doc, ev.props.Descriptor), null);
      if (child >= 0) out.push({ type: 'spawner', child, metric: toSym(ev.props.SpawnMetric) || 'Distance', interval: num(ev.props.SpawnInterval, 0.05) });
      break;
    }
    case 'CParticleEvolver_Localspace':
      // run children evolvers inline (local-space transform ignored for a static emitter)
      for (const ref of ev.props.ChildList || []) addEvolver(ctx, deref(doc, ref), out, samplers);
      break;
    // Collisions, Attractor, Projection, Flocking, etc: unsupported in v1
    default:
      out.push({ type: 'unsupported', cls: ev.className });
  }
}

function collectRenderers(doc, node, out, depth = 0) {
  if (!node || depth > 8) return;
  if (node.className === 'CParticleRenderer_List') {
    for (const ref of node.props.Renderers || []) collectRenderers(doc, deref(doc, ref), out, depth + 1);
    return;
  }
  if (node.className === 'CParticleRenderer_Billboard') {
    out.push({
      kind: 'billboard',
      // PopcornFX v1 default billboard material is Additive (glow textures with a
      // black background omit BillboardingMaterial and rely on additive blending).
      material: toSym(node.props.BillboardingMaterial) || 'Additive',
      mode: toSym(node.props.BillboardMode) || 'ScreenAlignedQuad',
      diffuse: node.props.Diffuse || null,
      atlas: node.props.AtlasDefinition || null,
      axisField: node.props.AxisField || null,
      axis2Field: node.props.Axis2Field || null,
      sizeField: node.props.SizeField || 'Size',
      colorField: node.props.ColorField || 'Color',
      rotationField: 'Rotation',
      drawOrder: num(node.props.DrawOrder, 0),
      axisScale: num(node.props.AxisScale, 1),
      softness: num(node.props.SoftnessDistance, 0),
    });
    return;
  }
  if (node.className === 'CParticleRenderer_Ribbon') {
    out.push({
      kind: 'ribbon',
      material: toSym(node.props.BillboardingMaterial) || 'Additive',
      diffuse: node.props.Diffuse || null,
      atlas: node.props.AtlasDefinition || null,
      colorField: node.props.ColorField || 'Color',
      sizeField: node.props.SizeField || 'Size',
      textureUField: node.props.TextureUField || null,  // e.g. "LifeRatio"
      drawOrder: num(node.props.DrawOrder, 0),
    });
    return;
  }
  if (node.className === 'CParticleRenderer_Mesh') {
    // Mesh particles render as flat-shaded box proxies (no .pkmm decode yet), sized/
    // colored/oriented per particle — enough to show where + how the mesh particles move.
    const md = deref(doc, (node.props.Meshes || [])[0]);
    out.push({
      kind: 'mesh',
      proxy: true,
      diffuseColor: (md && toNums(md.props.DiffuseColor)) || [0.8, 0.8, 0.85],
      scale: toNums(node.props.Scale) || [1, 1, 1],
      scaleField: node.props.ScaleField || 'Size',
      colorField: node.props.ColorField || 'Color',
      rotationField: 'Rotation',
      drawOrder: num(node.props.DrawOrder, 0),
    });
    return;
  }
  if (node.className === 'CParticleRenderer_Light') {
    // Lights have no lit geometry to illuminate in a particle preview, so we render an
    // additive glow stand-in at the light's position (color/radius).
    out.push({
      kind: 'light',
      colorField: node.props.ColorField || 'Color',
      radius: num(node.props.ConstantRadius, 1),
      radiusField: node.props.RadiusField || null,
      intensity: num(node.props.LightIntensityMultiplier, 1),
      drawOrder: num(node.props.DrawOrder, -2),
    });
    return;
  }
  // Decal / Sound / etc: recorded as unsupported
  out.push({ kind: 'unsupported', cls: node.className });
}

function tryCompile(src) { try { return compileScript(src); } catch (e) { console.warn('script compile failed:', e.message); return null; } }
function num(v, d) { return typeof v === 'number' ? v : (v == null ? d : (toNums(v)?.[0] ?? d)); }

// Normalize a parsed .pkfx document into a runtime Effect: a list of particle
// Layers, each with resolved fields (+ transform filters), samplers, a compiled
// spawn script, an ordered evolver tree, spawn/emission specs, events, and a
// flat list of renderers.
//
// Serialization omits default-valued properties, so absent props carry meaning:
//   SpawnCountMode absent      -> particles-per-second (TotalParticleCount only when written)
//   DurationInSeconds absent   -> 1.0s
//   ContinuousSpawner absent   -> true (false = pulse bursts every period)
//   SpawnMetric absent         -> Distance (trail evolvers; Time only when written)
//   SampleDimensionality absent-> Surface
//   BillboardMode absent       -> ScreenAlignedQuad
import { deref, toNums, toSym } from './parser.js';
import { compileScript } from './script.js';
import { CurveSampler, DoubleCurveSampler, ShapeSampler, TurbulenceSampler } from './curves.js';

const FIELD_COMP = { float: 1, float2: 2, float3: 3, float4: 4, int: 1, int2: 2, int3: 3, int4: 4 };
// Built-in fields every layer has, with default component counts. The __ fields are
// runtime bookkeeping: per-particle random seed, spawner LifeRatio/EmittedCount/Age.
const BUILTINS = { Life: 1, Age: 1, Position: 3, Velocity: 3, Size: 2, Color: 4, Rotation: 1, TextureID: 1, __rand: 1, __sLR: 1, __sEC: 1, __sAge: 1 };

// Billboard mode -> renderer geometry program:
// 0 screen-aligned, 1 viewpos-aligned, 2 axis-stretched, 3 axis-spheroidal, 4 planar
const BB_MODE = {
  ScreenAlignedQuad: 0, ScreenPoint: 0, ViewposAlignedQuad: 1,
  VelocityAxisAligned: 2, VelocityCapsuleAlign: 2, VelocitySpheroidalAlign: 3,
  PlanarAlignedQuad: 4, NormalAxisAligned: 2, SideAxisAligned: 2,
};

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
      attributes[a.props.AttributeName] = trimVec(toNums(a.props.DefaultValueF4 || a.props.DefaultValueI4) || [0, 0, 0, 0]);
    }
  }

  // Build the layer TREE: root layers (OnSpawn) + child layers reached via sub-emitters
  // (trail spawner-evolvers and events). Layers reference children BY INDEX, so a
  // parent particle can spawn into its child layer at runtime.
  const ctx = { doc, rng, globalSamplers, attributes, layers: [], indexByDesc: new Map(), groupCount: 0 };
  const rootSpawners = [];
  collectSpawners(doc, deref(doc, root.props.OnSpawn), rootSpawners, 0, 0, null, ctx);
  for (const sp of rootSpawners) ensureLayer(ctx, deref(doc, sp.node.props.Descriptor), sp);

  return {
    root, layers: ctx.layers, attributes,
    randomGroups: ctx.groupCount,
    looping: toSym(deref(doc, root.props.OnSpawn)?.props?.IsLooping),
  };
}

// Build a layer for a descriptor if not already built; return its index. `spawner` is
// {node, delay, group} (null for child layers, which only spawn via a parent). Reserves
// the index BEFORE recursing into children so cycles terminate.
function ensureLayer(ctx, desc, spawner) {
  if (!desc) return -1;
  if (ctx.indexByDesc.has(desc.id)) {
    const idx = ctx.indexByDesc.get(desc.id);
    // a child layer can also be root-spawned; attach the spawn spec if it arrives later
    const l = ctx.layers[idx];
    if (l && spawner && !l.spawn) { l.spawn = spawnSpec(ctx, spawner); l.isChild = false; }
    return idx;
  }
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

// Walk an action tree down to particle spawners, accumulating WithChilds delays.
// WithRandomChilds children become alternatives of a random group: at reset the
// runtime picks ONE per group (weighted) instead of firing all of them.
function collectSpawners(doc, node, out, depth, delay, group, ctx) {
  if (!node || depth > 32) return;
  const cn = node.className;
  const ownDelay = num(node.props.Delay, 0);
  if (cn === 'CActionFactoryParticleSpawnerBase') {
    out.push({ node, delay: delay + ownDelay, group });
    return;
  }
  if (cn === 'CActionFactoryWithChilds') {
    for (const ref of node.props.ChildList || []) collectSpawners(doc, deref(doc, ref), out, depth + 1, delay + ownDelay, group, ctx);
    return;
  }
  if (cn === 'CActionFactoryWithRandomChilds') {
    const gid = ctx ? ctx.groupCount++ : 0;
    const kids = node.props.ChildList || [];
    kids.forEach((ref, alt) => {
      const child = deref(doc, ref);
      if (!child) return;
      const weight = num(child.props.Weight, 1);
      collectSpawners(doc, child, out, depth + 1, delay + ownDelay, { id: gid, alt, weight }, ctx);
    });
    return;
  }
  // other action factories (sound, entity) are ignored for rendering
}

// Spawn spec for one spawner action. Defaults follow what the corpus never writes.
function spawnSpec(ctx, spawner) {
  const p = spawner.node.props;
  return {
    count: num(p.SpawnCount, 10),
    duration: num(p.DurationInSeconds, 1),
    totalMode: toSym(p.SpawnCountMode) === 'TotalParticleCount',
    infinite: p.Infinite === true,
    continuous: p.ContinuousSpawner !== false,
    delay: spawner.delay,   // already includes this spawner's own Delay + parent WithChilds delays
    randomDelay: num(p.RandomDelay, 0),
    firstDelay: num(p.FirstSpawnDelay, 0),
    countDeviation: num(p.SpawnCountRelativeRandomDeviation, 0),
    durationDeviation: num(p.DurationRelativeRandomDeviation, 0),
    fluxAttr: typeof p.FluxFactorExpression === 'string' ? p.FluxFactorExpression : null,
    fluxCurve: samplerFor(ctx.doc, p.FluxFunction, ctx.rng),
    fluxTile: num(p.FluxFunctionTiledRelativeDuration, 0),
    group: spawner.group,
  };
}

function buildLayer(ctx, desc, spawner) {
  const { doc, rng, globalSamplers } = ctx;
  // ---- fields ----
  const fields = []; const fieldIndex = {};
  const addField = (name, comp, tf) => {
    if (fieldIndex[name]) { if (tf && !fieldIndex[name].tf) fieldIndex[name].tf = tf; return; }
    fieldIndex[name] = { offset: 0, comp, tf: tf || null }; fields.push({ name, comp });
  };
  for (const [name, comp] of Object.entries(BUILTINS)) addField(name, comp);
  for (const ref of desc.props.CustomFields || []) {
    const f = deref(doc, ref); if (!f) continue;
    const comp = FIELD_COMP[toSym(f.props.FieldType)] ?? 1;
    addField(f.props.FieldName, comp, toSym(f.props.TransformFilter));
  }

  // ---- samplers (descriptor-local + global) ----
  const samplers = Object.assign({}, globalSamplers);
  for (const ref of desc.props.Samplers || []) addSampler(doc, deref(doc, ref), samplers, rng);

  // ---- spawn script ----
  let spawnScript = null;
  const se = deref(doc, desc.props.SpawnEvaluator);
  if (se && typeof se.props.Expression === 'string') spawnScript = tryCompile(se.props.Expression);

  // ---- evolvers (ordered tree; localspace keeps its children nested) ----
  const layerCtx = { ctx, samplers, fieldIndex, addField, spawnerAcc: 0 };
  const evolvers = [];
  const state = deref(doc, (desc.props.States || [])[0]);
  if (state) for (const ref of state.props.Evolvers || []) addEvolver(layerCtx, deref(doc, ref), evolvers);

  // ---- events: EventName -> [{ child layer index, spawn spec }] ----
  const events = {};
  for (const ref of desc.props.CustomEvents || []) {
    const ed = deref(doc, ref); if (!ed || !ed.props.EventName) continue;
    const evSpawners = [];
    collectSpawners(doc, deref(doc, ed.props.EventAction), evSpawners, 0, 0, null, ctx);
    const targets = [];
    for (const sp of evSpawners) {
      const cd = deref(doc, sp.node.props.Descriptor); if (!cd) continue;
      targets.push({ layer: ensureLayer(ctx, cd, null), spec: spawnSpec(ctx, sp) });
    }
    if (targets.length) events[ed.props.EventName] = (events[ed.props.EventName] || []).concat(targets);
  }

  // ---- renderers (flattened) ----
  const renderers = [];
  collectRenderers(doc, deref(doc, desc.props.Renderer), renderers, fieldIndex);

  // assign field offsets (SoA stride) — after evolvers may have added scratch fields
  let stride = 0; for (const f of fields) { fieldIndex[f.name].offset = stride; stride += f.comp; }

  return {
    name: (spawner ? spawner.node.id : desc.id).replace('$LOCAL$/', ''),
    isChild: !spawner,
    fields, fieldIndex, stride,
    samplers, spawnScript, evolvers, events, renderers,
    spawn: spawner ? spawnSpec(ctx, spawner) : null,
    inheritVelocity: num(desc.props.InheritInitialVelocity, 0),
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
    case 'CParticleSamplerShape': {
      const s = new ShapeSampler(deref(doc, obj.props.Shape), rng);
      s.volume = toSym(obj.props.SampleDimensionality) === 'Volume';
      return s;
    }
    case 'CParticleSamplerProceduralTurbulence': return new TurbulenceSampler(obj);
    default: return { sample: () => [0] };
  }
}
function samplerFor(doc, ref, rng) {
  const obj = deref(doc, ref);
  return obj ? makeSampler(doc, obj, rng) : null;
}

function addEvolver(lc, ev, out) {
  if (!ev) return;
  if (ev.props && ev.props.Active === false) return; // disabled in the editor
  const { ctx, samplers, fieldIndex } = lc;
  const { doc, rng } = ctx;
  switch (ev.className) {
    case 'CParticleEvolver_Physics':
      out.push({
        type: 'physics',
        accel: toNums(ev.props.ConstantAcceleration) || [0, 0, 0],
        drag: num(ev.props.Drag, 0),
        mass: Math.max(num(ev.props.Mass, 1), 1e-3),
        constVel: toNums(ev.props.ConstantVelocityField) || null,
        velField: typeof ev.props.VelocityFieldSampler === 'string' ? ev.props.VelocityFieldSampler : null,
        posField: fieldName(ev.props.PositionField, 'Position'),
        velName: fieldName(ev.props.VelocityField, 'Velocity'),
      });
      break;
    case 'CParticleEvolver_Field': {
      const curve = makeSampler(doc, deref(doc, ev.props.Evaluator), rng);
      out.push({ type: 'field', field: ev.props.Name, curve });
      break;
    }
    case 'CParticleEvolver_Script': {
      const expr = deref(doc, ev.props.Expression);
      const sc = expr && typeof expr.props.Expression === 'string' ? tryCompile(expr.props.Expression) : null;
      if (sc) out.push({ type: 'script', script: sc, errors: 0 });
      break;
    }
    case 'CParticleEvolver_FlipBook': {
      const cursor = fieldName(ev.props.AnimationCursor, null);
      out.push({
        type: 'flipbook',
        first: num(ev.props.FirstFrameID, 0),
        last: num(ev.props.LastFrameID, 0),
        loop: Math.max(num(ev.props.LoopCount, 1), 1e-3),
        cursorField: cursor && fieldIndex[cursor] ? cursor : null,
        randomize: ev.props.RandomizeFirstFrame === true,
        outField: fieldName(ev.props.OutputFrameID, 'TextureID'),
      });
      break;
    }
    case 'CParticleEvolver_Rotation':
      // rotation speed is a per-particle FIELD (set by the spawn script), not a prop
      out.push({
        type: 'rotation',
        speedField: fieldName(ev.props.ScalarRotationSpeedField, 'ScalarRotationSpeed'),
        angleField: fieldName(ev.props.RotationAngleField, 'Rotation'),
      });
      break;
    case 'CParticleEvolver_Damper':
      out.push({
        type: 'damper',
        field: ev.props.FieldToDampen,
        time: num(ev.props.ExpDampingTime, 0.1),
        minSpeed: num(ev.props.MinSpeed, 0),
      });
      break;
    case 'CParticleEvolver_Spawner': {
      // a trail: each parent particle emits into the child layer over time/distance
      const child = ensureLayer(ctx, deref(doc, ev.props.Descriptor), null);
      if (child >= 0) {
        // per-particle scratch: metric accumulator (+ previous position for Distance)
        const accField = `__sp${lc.spawnerAcc++}`;
        lc.addField(accField, 1);
        lc.addField('__prev', 3);
        out.push({
          type: 'spawner', child,
          metric: toSym(ev.props.SpawnMetric) || 'Distance',
          interval: Math.max(num(ev.props.SpawnInterval, 0.05), 1e-4),
          firstDelay: num(ev.props.FirstSpawnDelay, 0),
          accField,
        });
      }
      break;
    }
    case 'CParticleEvolver_Localspace': {
      // particles inside the block live in emitter space. With the default
      // enter=Previous / leave=Current transforms, each frame applies the emitter's
      // movement delta -> the particles are attached to (follow) the emitter. An
      // explicitly written mode pair cancels out (local axes only, no attachment).
      const children = [];
      for (const ref of ev.props.ChildList || []) addEvolver(lc, deref(doc, ref), children);
      const enterPrev = toSym(ev.props.ModeEnter) !== 'WorldToLocal_Current';
      const leaveCur = toSym(ev.props.ModeLeave) !== 'LocalToWorld_Previous';
      const neutral = ev.props.UseEffectTransforms === false || ev.props.TransformTranslate === false;
      // per-frame delta factor: leaveTransform - enterTransform
      const attach = neutral ? 0 : (enterPrev && leaveCur) ? 1 : (!enterPrev && !leaveCur) ? -1 : 0;
      out.push({ type: 'localspace', children, attach });
      break;
    }
    case 'CParticleEvolver_Attractor':
      out.push({
        type: 'attractor',
        shape: typeof ev.props.Shape === 'string' ? ev.props.Shape : null,
        force: num(ev.props.ForceAtSurface, 1),
        influence: num(ev.props.InfluenceDistance, 0),
        repulse: ev.props.RepulseWhenInside === true,
      });
      break;
    // Collisions, Projection, Flocking, etc: unsupported
    default:
      out.push({ type: 'unsupported', cls: ev.className });
  }
}

// a prop that names a field; empty strings and absent -> fallback
function fieldName(v, fallback) {
  return typeof v === 'string' && v ? v : fallback;
}

function collectRenderers(doc, node, out, fieldIndex, depth = 0) {
  if (!node || depth > 8) return;
  if (node.className === 'CParticleRenderer_List') {
    for (const ref of node.props.Renderers || []) collectRenderers(doc, deref(doc, ref), out, fieldIndex, depth + 1);
    return;
  }
  if (node.className === 'CParticleRenderer_Billboard') {
    const mode = toSym(node.props.BillboardMode) || 'ScreenAlignedQuad';
    out.push({
      kind: 'billboard',
      // PopcornFX v1 default billboard material is Additive (glow textures with a
      // black background omit BillboardingMaterial and rely on additive blending).
      material: toSym(node.props.BillboardingMaterial) || 'Additive',
      mode: BB_MODE[mode] ?? 0,
      modeName: mode,
      diffuse: node.props.Diffuse || null,
      atlas: node.props.AtlasDefinition || null,
      axisField: fieldName(node.props.AxisField, null),
      axis2Field: fieldName(node.props.Axis2Field, null),
      sizeField: fieldName(node.props.SizeField, 'Size'),
      colorField: fieldName(node.props.ColorField, 'Color'),
      rotationField: fieldName(node.props.RotationField, 'Rotation'),
      positionField: fieldName(node.props.PositionField, 'Position'),
      textureIDField: fieldName(node.props.TextureIDField, 'TextureID'),
      drawOrder: num(node.props.DrawOrder, 0),
      axisScale: num(node.props.AxisScale, 1),
      softness: num(node.props.SoftnessDistance, 0),
      softAnim: node.props.SoftAnimationBlending === true,
      alphaRemap: node.props.AlphaRemapper || null,
      alphaCursorField: fieldName(node.props.AlphaCursorField, null),
      vflip: node.props.VFlipUVs === true,
      aspect: num(node.props.AspectRatio, 1),
    });
    return;
  }
  if (node.className === 'CParticleRenderer_Ribbon') {
    out.push({
      kind: 'ribbon',
      material: toSym(node.props.BillboardingMaterial) || 'Additive',
      diffuse: node.props.Diffuse || null,
      atlas: node.props.AtlasDefinition || null,
      colorField: fieldName(node.props.ColorField, 'Color'),
      sizeField: fieldName(node.props.SizeField, fieldName(node.props.WidthField, 'Size')),
      width: num(node.props.Width, 0),
      textureUField: node.props.TextureUField || null,  // e.g. "LifeRatio"
      positionField: fieldName(node.props.PositionField, 'Position'),
      drawOrder: num(node.props.DrawOrder, 0),
    });
    return;
  }
  if (node.className === 'CParticleRenderer_Mesh') {
    // Real .pkmm geometry (decoded client-side); box proxy if the mesh is missing.
    const md = deref(doc, (node.props.Meshes || [])[0]);
    const mp = md ? md.props : {};
    out.push({
      kind: 'mesh',
      mesh: mp.Mesh || null,
      diffuse: mp.Diffuse || null,
      material: toSym(mp.Material) || 'Solid',
      diffuseColor: toNums(mp.DiffuseColor) || [1, 1, 1],
      scale: toNums(node.props.Scale) || [1, 1, 1],
      scaleField: fieldName(node.props.ScaleField, null),
      colorField: fieldName(node.props.ColorField, 'Color'),
      positionField: fieldName(node.props.PositionField, 'Position'),
      forwardAxisField: fieldName(node.props.ForwardAxisField, null),
      upAxisField: fieldName(node.props.UpAxisField, null),
      eulerRotationField: fieldName(node.props.EulerRotationField, null),
      rotationAxisField: fieldName(node.props.RotationAxisField, null),
      rotationAxisAngleField: fieldName(node.props.RotationAxisAngleField, null),
      staticOrientation: toNums(node.props.StaticOrientationOffset) || null,   // euler degrees
      staticPosition: toNums(node.props.StaticPositionOffset) || null,
      drawOrder: num(node.props.DrawOrder, 0),
    });
    return;
  }
  if (node.className === 'CParticleRenderer_Light') {
    // Lights have no lit geometry to illuminate in a particle preview, so we render an
    // additive glow stand-in at the light's position (color/radius).
    out.push({
      kind: 'light',
      colorField: fieldName(node.props.ColorField, 'Color'),
      radius: num(node.props.ConstantRadius, 1) * num(node.props.LightRadiusMultiplier, 1),
      intensity: num(node.props.LightIntensityMultiplier, 1),
      drawOrder: num(node.props.DrawOrder, -2),
    });
    return;
  }
  if (node.className === 'CParticleRenderer_Null') return;
  // Decal / Sound / etc: recorded as unsupported
  out.push({ kind: 'unsupported', cls: node.className });
}

function tryCompile(src) { try { return compileScript(src); } catch (e) { console.warn('script compile failed:', e.message); return null; } }
function num(v, d) { return typeof v === 'number' ? v : (v == null ? d : (toNums(v)?.[0] ?? d)); }

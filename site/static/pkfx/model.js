// Normalize a parsed .pkfx document into a runtime Effect: a list of particle
// Layers, each with resolved fields (+ transform filters), samplers, a compiled
// spawn script, an ordered evolver tree, spawn/emission specs, events, and a
// flat list of renderers.
//
// Serialization omits default-valued properties, so absent props carry meaning:
//   SpawnCountMode absent      -> particles-per-second (TotalParticleCount only when written)
//   DurationInSeconds absent   -> 0 = an instant burst (unless Infinite)
//   SpawnCount absent          -> 1
//   SpawnMetric absent         -> Distance (trail evolvers; Time only when written)
//   SampleDimensionality absent-> Surface
//   BillboardMode absent       -> ScreenAlignedQuad
import { deref, toNums, toSym } from './parser.js';
import { compileScript } from './script.js';
import { AnimTrackSampler, CurveSampler, DoubleCurveSampler, ShapeSampler, TurbulenceSampler } from './curves.js';

const FIELD_COMP = { float: 1, float2: 2, float3: 3, float4: 4, int: 1, int2: 2, int3: 3, int4: 4 };
// Built-in fields every layer has, with default component counts. The __ fields are
// runtime bookkeeping: per-particle random seed, spawner LifeRatio/EmittedCount/Age,
// a flag + in-frame dt for particles still in their spawn frame, and the SelfID /
// ribbon group (spawner or parent) that ribbons link by.
const BUILTINS = { Life: 1, Age: 1, Position: 3, Velocity: 3, Size: 2, Color: 4, Rotation: 1, TextureID: 1, __rand: 1, __sLR: 1, __sEC: 1, __sAge: 1, __born: 1, __dt: 1, __sid: 1, __grp: 1 };

// Billboard mode -> renderer geometry program:
// 0 screen-aligned, 1 viewpos-aligned, 2 axis-stretched, 3 axis-spheroidal, 4 planar, 5 capsule
const BB_MODE = {
  ScreenAlignedQuad: 0, ScreenPoint: 0, ViewposAlignedQuad: 1,
  VelocityAxisAligned: 2, VelocityCapsuleAlign: 5, VelocitySpheroidalAlign: 3,
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
  collectSpawners(doc, deref(doc, root.props.OnSpawn), rootSpawners, 0, [], null, ctx);
  for (const sp of rootSpawners) ensureLayer(ctx, deref(doc, sp.node.props.Descriptor), sp);

  return { root, layers: ctx.layers, attributes, randomGroups: ctx.groupCount };
}

// Build a layer for a descriptor if not already built; return its index. `spawner` is
// {node, delays, group} (null for child layers, which only spawn via a parent). Reserves
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

// Walk an action tree down to particle spawners, collecting each node's Delay and
// RandomDelay (Delay * U[1-r, 1+r], rolled per start) down the chain.
// WithRandomChilds children become alternatives of a random group: at reset the
// runtime picks ONE per group (weighted) instead of firing all of them.
function collectSpawners(doc, node, out, depth, delays, group, ctx) {
  if (!node || depth > 32) return;
  const cn = node.className;
  const d = num(node.props.Delay, 0);
  const chain = d > 0 ? delays.concat([[d, Math.min(Math.max(num(node.props.RandomDelay, 0), 0), 1)]]) : delays;
  if (cn === 'CActionFactoryParticleSpawnerBase') {
    out.push({ node, delays: chain, group });
    return;
  }
  if (cn === 'CActionFactoryWithChilds') {
    for (const ref of node.props.ChildList || []) collectSpawners(doc, deref(doc, ref), out, depth + 1, chain, group, ctx);
    return;
  }
  if (cn === 'CActionFactoryWithRandomChilds') {
    const gid = ctx ? ctx.groupCount++ : 0;
    const kids = node.props.ChildList || [];
    kids.forEach((ref, alt) => {
      const child = deref(doc, ref);
      if (!child) return;
      const weight = num(child.props.Weight, 1);
      collectSpawners(doc, child, out, depth + 1, chain, { id: gid, alt, weight }, ctx);
    });
    return;
  }
  // other action factories (sound, entity) are ignored for rendering
}

// Spawn spec for one spawner action (CActionFactoryParticleSpawnerBase defaults).
// Duration 0 without Infinite is an instant burst; Infinite ignores TotalParticleCount.
// ContinuousSpawner only lerps spawn positions along the emitter's motion.
function spawnSpec(ctx, spawner) {
  const p = spawner.node.props;
  return {
    count: num(p.SpawnCount, 1),
    infinite: p.Infinite === true,
    duration: p.Infinite === true ? Infinity : Math.max(num(p.DurationInSeconds, 0), 0),
    totalMode: toSym(p.SpawnCountMode) === 'TotalParticleCount',
    interpolate: p.ContinuousSpawner !== false,
    delays: spawner.delays,
    firstDelay: Math.min(Math.max(num(p.FirstSpawnDelay, 0), 0), 1),   // fraction of one interval
    countDeviation: num(p.SpawnCountRelativeRandomDeviation, 0),
    durationDeviation: num(p.DurationRelativeRandomDeviation, 0),
    fluxAttr: typeof p.FluxFactorExpression === 'string' ? p.FluxFactorExpression : null,
    fluxCurve: samplerFor(ctx.doc, p.FluxFunction, ctx.rng),
    fluxTile: num(p.FluxFunctionTiledRelativeDuration, 1),
    fluxIntegrate: p.FluxFunction_ComputeIntegrals !== false,
    fluxTiling: p.FluxFunction_EnableTiling !== false,
    fluxDiscrete: p.FluxFunction_DiscreteSpawnKeys === true,
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
  // samplerRefs: LimitDistance binds its sampler by object, not by name
  const layerCtx = { ctx, samplers, fieldIndex, addField, spawnerAcc: 0, samplerRefs: new Set(desc.props.Samplers || []) };
  const evolvers = [];
  const state = deref(doc, (desc.props.States || [])[0]);
  if (state) for (const ref of state.props.Evolvers || []) addEvolver(layerCtx, deref(doc, ref), evolvers);

  // ---- events: EventName -> [{ child layer index, spawn spec }] ----
  const events = {};
  for (const ref of desc.props.CustomEvents || []) {
    const ed = deref(doc, ref); if (!ed || !ed.props.EventName) continue;
    const evSpawners = [];
    collectSpawners(doc, deref(doc, ed.props.EventAction), evSpawners, 0, [], null, ctx);
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
      const s = new ShapeSampler(deref(doc, obj.props.Shape), rng, doc);
      s.volume = toSym(obj.props.SampleDimensionality) === 'Volume';
      s.translate = obj.props.TransformTranslate !== false;
      s.rotate = obj.props.TransformRotate !== false;
      return s;
    }
    case 'CParticleSamplerProceduralTurbulence': return new TurbulenceSampler(obj);
    case 'CParticleSamplerAnimTrack': return new AnimTrackSampler(obj);
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
      // Mass is INVERSE mass (1/m) and 0 means no drag; a layer field named by MassField
      // overrides it per particle. Accel and Force fields add to the acceleration.
      out.push({
        type: 'physics',
        accel: toNums(ev.props.ConstantAcceleration) || [0, 0, 0],
        drag: num(ev.props.Drag, 0),
        mass: num(ev.props.Mass, 1),
        constVel: toNums(ev.props.ConstantVelocityField) || null,
        velField: typeof ev.props.VelocityFieldSampler === 'string' ? ev.props.VelocityFieldSampler : null,
        posField: fieldName(ev.props.PositionField, 'Position'),
        velName: fieldName(ev.props.VelocityField, 'Velocity'),
        massField: fieldName(ev.props.MassField, 'Mass'),
        accelField: fieldName(ev.props.AccelField, 'Accel'),
        forceField: fieldName(ev.props.ForceField, 'Force'),
        windField: fieldName(ev.props.VelocityFieldField, 'VelocityField'),
      });
      // WorldInteractionMode appends the shared collision kernel right after physics
      if ({ OneWay: 1, TwoWay: 2 }[toSym(ev.props.WorldInteractionMode)]) {
        lc.addField('__cflags', 1); lc.addField('__prev', 3);
        out.push(collideSpec(ev.props, true, null));
      }
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
      // Output is a float frame (the renderer floors it / soft-blends the fraction). A
      // cursor naming no field ("0", "") never animates: the frame keeps its spawn value.
      const cursor = fieldName(ev.props.AnimationCursor, 'LifeRatio');
      const first = num(ev.props.FirstFrameID, 0), last = num(ev.props.LastFrameID, 1);
      out.push({
        type: 'flipbook',
        base: first,
        scale: last >= first ? (last - first) + 0.9999 : -((first - last) + 0.9999),
        loop: num(ev.props.LoopCount, 1),
        cursor,
        cursorOk: cursor === 'LifeRatio' || !!fieldIndex[cursor],
        randomize: ev.props.RandomizeFirstFrame === true,
        outField: fieldName(ev.props.OutputFrameID, 'TextureID'),
      });
      break;
    }
    case 'CParticleEvolver_Rotation':
      // rotation speed is a per-particle FIELD (radians/s), scaled by ScreenspaceRotationCoeff
      out.push({
        type: 'rotation',
        axial: toSym(ev.props.RotationMode) === 'Axial',
        coeff: num(ev.props.ScreenspaceRotationCoeff, 1),
        speedField: fieldName(ev.props.ScalarRotationSpeedField, 'ScalarRotationSpeed'),
        axialField: fieldName(ev.props.AxialRotationSpeedField, 'RotationSpeed'),
        angleField: fieldName(ev.props.RotationAngleField, 'Rotation'),
        posField: fieldName(ev.props.PositionField, 'Position'),
      });
      break;
    case 'CParticleEvolver_Damper':
      // ExpDampingTime is a RATE (v *= exp(-rate*dt)); 0 = no damping. MinSpeed is a floor.
      out.push({
        type: 'damper',
        field: fieldName(ev.props.FieldToDampen, 'RotationSpeed'),
        rate: Math.abs(num(ev.props.ExpDampingTime, 0)),
        minSpeed: Math.abs(num(ev.props.MinSpeed, 0)),
      });
      break;
    case 'CParticleEvolver_Spawner': {
      // a trail: each parent particle emits into the child layer over time/distance
      const child = ensureLayer(ctx, deref(doc, ev.props.Descriptor), null);
      if (child >= 0) {
        // per-particle scratch: interval accumulator, emitted counter, previous position
        const n = lc.spawnerAcc++;
        const accField = `__sp${n}`, countField = `__spc${n}`;
        lc.addField(accField, 1);
        lc.addField(countField, 1);
        lc.addField('__prev', 3);
        out.push({
          type: 'spawner', child,
          metric: toSym(ev.props.SpawnMetric) || 'Distance',
          interval: Math.max(num(ev.props.SpawnInterval, 0.1), 1e-5),
          firstDelay: Math.min(Math.max(num(ev.props.FirstSpawnDelay, 1), 0), 1),   // fraction of one interval
          flux: samplerFor(doc, ev.props.FluxFunction, rng),
          tile: num(ev.props.FluxFunctionTiledRelativeDuration, 1),
          accField, countField,
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
      // writes the Force field (surface-projected, with falloff); Physics applies it
      lc.addField(fieldName(ev.props.ForceField, 'Force'), 3);
      out.push({
        type: 'attractor',
        shape: typeof ev.props.Shape === 'string' ? ev.props.Shape : null,
        force: num(ev.props.ForceAtSurface, 1),
        finite: toSym(ev.props.FalloffType) === 'Finite',
        influence: num(ev.props.InfluenceDistance, 10),
        steepness: num(ev.props.FalloffSteepness, 8),
        repulse: ev.props.RepulseWhenInside === true,
        posField: fieldName(ev.props.PositionField, 'Position'),
        forceField: fieldName(ev.props.ForceField, 'Force'),
      });
      break;
    case 'CParticleEvolver_Collisions':
      lc.addField('__cflags', 1); lc.addField('__prev', 3);
      out.push(collideSpec(ev.props, false, typeof ev.props.Collider === 'string' && ev.props.Collider ? ev.props.Collider : null));
      break;
    case 'CParticleEvolver_Projection': {
      // moves Position onto the shape's surface each frame; optional int3 pcoords out
      const pcField = fieldName(ev.props.OutputParametricCoordsField, null);
      if (pcField) lc.addField(pcField, 3);
      out.push({
        type: 'projection',
        shape: typeof ev.props.Shape === 'string' ? ev.props.Shape : null,
        posField: fieldName(ev.props.PositionField, 'Position'),
        pcField,
      });
      break;
    }
    case 'CParticleEvolver_LimitDistance': {
      // pulls Position toward the shape's surface: hard clamp of the signed distance to
      // [HardMin, HardMax], soft relaxation toward [MinDistance, MaxDistance]
      const ref = ev.props.DistanceSampler;
      const so = typeof ref === 'string' && lc.samplerRefs.has(ref) ? deref(doc, ref) : null;
      out.push({
        type: 'limitdist',
        sampler: (so && so.props.SamplerName) || null,
        posField: fieldName(ev.props.PositionField, 'Position'),
        h: num(ev.props.ParticleSamplingDistance, 0.01),
        softness: num(ev.props.DistancesSoftness, 5),
        max: num(ev.props.MaxDistance, 0),
        min: num(ev.props.MinDistance, -Infinity),
        hardMax: num(ev.props.HardMaxDistance, Infinity),
        hardMin: num(ev.props.HardMinDistance, -Infinity),
      });
      break;
    }
    case 'CParticleEvolver_SpatialInsertion': {
      const sl = spatialLayer(doc, ev.props.SpatialLayer);
      if (sl) out.push({ type: 'spatialinsert', layer: sl.name, fields: sl.fields, posField: fieldName(ev.props.PositionField, 'Position') });
      break;
    }
    case 'CParticleEvolver_Flocking': {
      const sl = spatialLayer(doc, ev.props.SpatialLayer);
      if (!sl) break;                                   // engine: must be bound to a spatial layer
      const F = num(ev.props.ForceMagnitude, 7);
      let mn = num(ev.props.MinSpeed, 1), mx = num(ev.props.MaxSpeed, 2);
      if (mx < mn) [mn, mx] = [mx, mn];
      const cosHalf = (deg) => Math.cos(deg * Math.PI / 360);
      const rS = num(ev.props.SeparationSearchRadius, 0.8), rA = num(ev.props.AlignmentSearchRadius, 1), rC = num(ev.props.CohesionSearchRadius, 1.7);
      out.push({
        type: 'flocking', layer: sl.name, minSpeed: mn, maxSpeed: mx,
        wS: num(ev.props.SeparationFactor, 0.5) * F, wA: num(ev.props.AlignmentFactor, 0.33) * F, wC: num(ev.props.CohesionFactor, 0.33) * F,
        r2: [rS * rS, rA * rA, rC * rC], rMax: Math.max(rS, rA, rC),
        cs: [cosHalf(num(ev.props.SeparationSearchAngle, 270)), cosHalf(num(ev.props.AlignmentSearchAngle, 90)), cosHalf(num(ev.props.CohesionSearchAngle, 190))],
        maxN: num(ev.props.MaxNeighborCount, -1),
        posField: fieldName(ev.props.PositionField, 'Position'),
        velField: fieldName(ev.props.VelocityField, 'Velocity'),
        meanField: fieldName(ev.props.MeanNeighborDirectionField, 'MeanNeighborDirection'),
      });
      break;
    }
    // unsupported evolvers are recorded and skipped
    default:
      out.push({ type: 'unsupported', cls: ev.className });
  }
}

// Collision response settings (CParticleEvolver_Collisions, or Physics' own collision
// props). Defaults differ between the two: ContactFriction 0.7 vs 0, DefaultMass vs Mass.
function collideSpec(p, physics, collider) {
  return {
    type: 'collide', collider,
    die: p.DieOnContact === true,
    maxBounces: num(p.BouncesBeforeDeath, 1),
    rest: num(p.BounceRestitution, 0.5),
    coulomb: toSym(p.ContactFrictionModel) === 'Coulomb',
    friction: num(p.ContactFriction, physics ? 0 : 0.7),
    restCombine: toSym(p.RestitutionCombineMode) || 'Surface',
    fricCombine: toSym(p.FrictionCombineMode) || 'Surface',
    ignoreSurface: p.IgnoreSurfaceProperties === true,
    offset: num(p.BounceOffset, 0.002),
    ndotv: p.WeightBounceWithNdotV === true,
    maxIter: Math.max(1, Math.min(6, Math.round(num(p.MaxIterations, 1)))),
    stopFinal: p.StopIfFinalIterationHits === true,
    event: typeof p.EventOnCollide === 'string' && p.EventOnCollide ? p.EventOnCollide : 'OnCollide',
    eventPostVel: p.EventUsesPostContactVelocity === true,
    mass: physics ? num(p.Mass, 1) : num(p.DefaultMass, 1),
    posField: fieldName(p.PositionField, 'Position'),
    velField: fieldName(p.VelocityField, 'Velocity'),
    massField: fieldName(p.MassField, 'Mass'),
    restField: fieldName(p.BounceResitutionField, 'BounceRestitution'),
    fricField: fieldName(p.ContactFrictionField, 'Friction'),
    countField: fieldName(p.CollisionCountField, 'CollisionCount'),
  };
}

// CParticleSpatialDescriptor -> { name, fields }: the stored fields besides Position
function spatialLayer(doc, ref) {
  const d = deref(doc, ref);
  const name = d && d.props.LayerName;
  if (!name) return null;
  const fields = [];
  for (const r of d.props.CustomFields || []) { const f = deref(doc, r); if (f && f.props.FieldName) fields.push(f.props.FieldName); }
  return { name, fields };
}

// a prop that names a field; empty strings and absent -> fallback
function fieldName(v, fallback) {
  return typeof v === 'string' && v ? v : fallback;
}

/* Trove reads a free-text `UserData` hint off the renderer, and the only one its
   shader acts on is `dissolve <width>` - the DissolveWidth uniform. 1,406 effects
   set it. Matched as the engine does, on the leading token, so the corpus's one
   "disssolve 0.1" typo stays off exactly as it does in game (its author's mistake
   is not ours to correct); the stray double space in ten others still parses. */
function dissolveWidth(v) {
  const m = typeof v === 'string' ? /^\s*dissolve\s+([0-9]*\.?[0-9]+)/i.exec(v) : null;
  return m ? parseFloat(m[1]) : 0;
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
      // Particles within one batch composite in draw order, so alpha-blended layers
      // must go back-to-front. The corpus writes SortMode in 37 of 9,369 effects
      // (FieldAscending/FieldDescending over SortField); everything else takes the
      // editor's default, CameraDistance.
      sortMode: toSym(node.props.SortMode) || 'CameraDistance',
      sortField: fieldName(node.props.SortField, null),
      axisScale: num(node.props.AxisScale, 0.1),
      constantRadius: num(node.props.ConstantRadius, 0),   // > 0 overrides the size field
      // only meaningful on a _Soft material; the editor shows 1 as its default
      softness: num(node.props.SoftnessDistance, 1),
      dissolve: dissolveWidth(node.props.UserData),
      softAnim: node.props.SoftAnimationBlending === true,
      alphaRemap: node.props.AlphaRemapper || null,
      alphaCursorField: fieldName(node.props.AlphaCursorField, null),
      vflip: node.props.VFlipUVs === true,
      aspect: num(node.props.AspectRatio, 1),
    });
    return;
  }
  if (node.className === 'CParticleRenderer_Ribbon') {
    // WidthField is a HALF width; the Width prop only applies when WidthField is "".
    const wf = node.props.WidthField;
    out.push({
      kind: 'ribbon',
      material: toSym(node.props.BillboardingMaterial) || 'Additive',
      mode: toSym(node.props.BillboardMode) || 'ViewposAligned',
      diffuse: node.props.Diffuse || null,
      atlas: node.props.AtlasDefinition || null,
      colorField: fieldName(node.props.ColorField, 'Color'),
      widthField: wf === '' ? null : fieldName(wf, 'Size'),
      width: num(node.props.Width, 1),
      axisField: fieldName(node.props.AxisField, null),
      textureUField: fieldName(node.props.TextureUField, null),
      textureID: num(node.props.TextureID, 0),
      textureIDField: fieldName(node.props.TextureIDField, 'TextureID'),
      flipU: node.props.FlipU === true,
      flipV: node.props.FlipV === true,
      rotateTexture: node.props.RotateTexture === true,
      repeat: node.props.TextureRepeat === true,
      softness: num(node.props.SoftnessDistance, 1),
      alphaRemap: node.props.AlphaRemapper || null,
      alphaCursorField: fieldName(node.props.AlphaCursorField, null),
      positionField: fieldName(node.props.PositionField, 'Position'),
      drawOrder: num(node.props.DrawOrder, 0),
    });
    return;
  }
  if (node.className === 'CParticleRenderer_Mesh') {
    // Real .pkmm geometry (decoded client-side); box proxy if the mesh is missing.
    const md = deref(doc, (node.props.Meshes || [])[0]);
    const mp = md ? md.props : {};
    // colour reaches a mesh only through a "DiffuseColor = <field>" material mapping
    let colorField = null;
    for (const m of mp.MaterialParametersFields || []) {
      const hit = typeof m === 'string' && /^\s*DiffuseColor\s*=\s*(\w+)/.exec(m);
      if (hit) colorField = hit[1];
    }
    out.push({
      kind: 'mesh',
      mesh: mp.Mesh || null,
      diffuse: mp.Diffuse || null,
      material: toSym(mp.Material) || 'Solid',
      diffuseColor: toNums(mp.DiffuseColor) || [1, 1, 1],
      scale: toNums(node.props.Scale) || [1, 1, 1],
      scaleField: fieldName(node.props.ScaleField, null),
      colorField,
      positionField: fieldName(node.props.PositionField, 'Position'),
      staticRotationAxis: toNums(node.props.StaticRotationAxis) || null,
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
  // Lights only illuminate lit scene geometry (a deferred splat); they draw nothing
  // themselves, and the preview has no lit surface for them to land on.
  if (node.className === 'CParticleRenderer_Light') return;
  if (node.className === 'CParticleRenderer_Null') return;
  // Decal / Sound / etc: recorded as unsupported
  out.push({ kind: 'unsupported', cls: node.className });
}

function tryCompile(src) { try { return compileScript(src); } catch (e) { console.warn('script compile failed:', e.message); return null; } }
function num(v, d) { return typeof v === 'number' ? v : (v == null ? d : (toNums(v)?.[0] ?? d)); }

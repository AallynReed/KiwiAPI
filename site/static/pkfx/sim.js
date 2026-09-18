// CPU particle simulation. One LayerSim per Layer holds a struct-of-arrays buffer
// (`data`, stride = sum of field components). Alive particles occupy [0, count);
// death is a swap-remove. Each frame: process emissions (spawn), then age + run
// the evolver pipeline. Particles born this frame evolve only for the part of the
// frame after their spawn instant (the engine's EvolveNewborns pass).
//
// Emissions are the spawn engine (CActionInstanceParticleSpawnerBase): a root layer
// gets one emission per loop, and events (OnDeath, script triggers) queue emissions at
// a world position. Duration 0 is an instant Burst; otherwise a Stream places its
// particles at even sub-frame instants, flux-modulated by an attribute and/or a curve.
import { TurbulenceSampler } from './curves.js';

const MAX = 20000;      // per-layer particle cap
const MAX_EMISSIONS = 128;

export class System {
  constructor(effect, rng = Math.random) {
    this.effect = effect;
    this.rng = rng;
    // World offset applied to root spawns. The preview moves this when the user drags
    // the emitter; localspace-attached layers follow it via emitterDelta.
    this.emitter = new Float32Array(3);
    this._emitterPrev = new Float32Array(3);
    this.emitterDelta = new Float32Array(3);
    // camera position, set by the viewer; axial Rotation evolvers read it
    this.camPos = null;
    this.attributes = effect.attributes || {};
    this.layers = effect.layers.map((l) => { const ls = new LayerSim(l, rng); ls.sys = this; ls.attributes = this.attributes; return ls; });
    // every turbulence sampler needs the running time for animation
    this._turb = [];
    for (const ls of this.layers) for (const s of Object.values(ls.L.samplers)) if (s instanceof TurbulenceSampler) this._turb.push(s);
    this.time = 0;
    this.clock = 0;   // scene.Time: total seconds, not reset when the effect restarts
    this.nextId = 0;  // SelfIDs, and (negated) spawner-instance ids for ribbon grouping
    this.reset();
  }

  reset() {
    this.time = 0;
    this.nextId = 0;
    // weighted pick per random-child group: only the chosen alternative spawns (weight 0 never)
    const picks = new Map();
    for (const l of this.layers) {
      const g = l.L.spawn && l.L.spawn.group;
      if (!g || picks.has(g.id)) continue;
      const members = this.layers.filter((o) => o.L.spawn && o.L.spawn.group && o.L.spawn.group.id === g.id);
      let total = 0; for (const m of members) total += m.L.spawn.group.weight;
      if (!(total > 0)) { picks.set(g.id, -1); continue; }
      let r = this.rng() * total;
      let chosen = members[members.length - 1];
      for (const m of members) { r -= m.L.spawn.group.weight; if (r < 0) { chosen = m; break; } }
      picks.set(g.id, chosen.L.spawn.group.alt);
    }
    for (const l of this.layers) {
      l.clear();
      const spec = l.L.spawn;
      if (!spec) continue;
      if (spec.group && picks.get(spec.group.id) !== spec.group.alt) continue;
      l.queueEmission(spec, null, null);
    }
  }

  update(dt) {
    this.time += dt;
    this.clock += dt;
    this.emitterDelta[0] = this.emitter[0] - this._emitterPrev[0];
    this.emitterDelta[1] = this.emitter[1] - this._emitterPrev[1];
    this.emitterDelta[2] = this.emitter[2] - this._emitterPrev[2];
    this._emitterPrev.set(this.emitter);
    for (const t of this._turb) t.time = this.time;

    for (const l of this.layers) l.spawnTick(dt);
    for (const l of this.layers) l.update(dt, false);
    // children spawned into a layer that had already updated this frame still get
    // their newborn evolve now, not a frame late
    for (let pass = 0; pass < 4; pass++) {
      let any = false;
      for (const l of this.layers) if (l._bornPending) { l._bornPending = false; any = true; l.update(dt, true); }
      if (!any) break;
    }
    let anyAlive = false, anyPending = false;
    for (const l of this.layers) {
      if (l.count > 0) anyAlive = true;
      if (l.emissions.length) anyPending = true;
    }
    if (!anyAlive && !anyPending) this.reset();
  }
}

class LayerSim {
  constructor(layer, rng) {
    this.L = layer; this.rng = rng;
    this.stride = layer.stride;
    this.data = new Float32Array(MAX * this.stride);
    this.count = 0;
    this.emissions = [];
    this._needsPrev = !!layer.fieldIndex.__prev;
    this._bornPending = false;
    this._ctx = this.makeCtx();
  }
  clear() { this.count = 0; this.emissions = []; }

  field(name) { return this.L.fieldIndex[name]; }

  // Start an emission of `spec` at world `origin` (null -> the moving emitter),
  // optionally seeding spawned particles with an inherited velocity.
  queueEmission(spec, origin, vel0, parentSnap) {
    if (this.emissions.length >= MAX_EMISSIONS) return;
    const rng = this.rng;
    let delay = 0;
    for (const [d, rd] of spec.delays) delay += d * (rd ? 1 + (rng() * 2 - 1) * rd : 1);
    let dur = spec.duration;
    if (isFinite(dur) && dur > 0 && spec.durationDeviation > 0) dur *= 1 + (rng() - 0.5) * spec.durationDeviation;
    let rate = spec.count;
    if (!spec.infinite && spec.totalMode && dur > 0) {
      rate = spec.fluxCurve && spec.fluxCurve.integral ? spec.count * this._curveI(spec) / dur : spec.count / dur;
    }
    this.emissions.push({
      id: -(++this.sys.nextId), spec, duration: dur, burst: dur === 0, rate,
      t: -delay, age: 0,
      acc: 1 - spec.firstDelay, roll: -1, emitted: 0,
      origin: origin ? [origin[0], origin[1], origin[2]] : null,
      vel0: vel0 || null,
      parentSnap: parentSnap || null,
    });
  }

  _curveI(spec) {
    const c = spec.fluxCurve, t = c.curve.t;
    if (spec._I == null) spec._I = t.length ? c.integral(t[0], t[t.length - 1]) : 0;
    return spec._I;
  }

  _attrFlux(spec) {
    if (!spec.fluxAttr) return 1;
    const a = this.attributes && this.attributes[spec.fluxAttr];
    // a declared default of 0 means "off until the game drives it" — preview at 1
    return a && a[0] ? a[0] : 1;
  }

  // Particles a stream owes this frame at its base rate (FUN_180740830): dt without a
  // curve; with one, the curve integrated over the frame (or sampled mid-frame, or its
  // keys summed), mapped so FluxFunctionTiledRelativeDuration repeats fit the duration.
  _curveFlux(e, dt) {
    const spec = e.spec, c = spec.fluxCurve;
    if (!c || !c.integral) return dt;
    const f = isFinite(e.duration) ? spec.fluxTile / e.duration : 1 / spec.fluxTile;
    if (!(f > 0) || !isFinite(f)) return dt;
    const frac = (x) => x - Math.floor(x);
    if (!spec.fluxIntegrate && !spec.fluxDiscrete) {
      let x = f * Math.min(e.age + dt * 0.5, e.duration);
      x = spec.fluxTiling ? frac(x) : Math.min(x, 1);
      return (c.sample([x])[0] || 0) * dt;
    }
    const x0 = spec.fluxTiling ? frac(f * e.age) : Math.min(f * e.age, 1);
    let x1 = x0 + f * dt;
    if (!spec.fluxTiling) x1 = Math.min(x1, 1);
    const part = spec.fluxDiscrete ? (a, b) => c.keySum(a, b) : (a, b) => c.integral(a, b);
    let sum;
    if (x1 <= 1) sum = part(x0, x1);
    else {
      const whole = Math.floor(x1) - 1;
      sum = part(x0, 1) + whole * this._curveI(spec) + part(0, x1 - Math.floor(x1));
    }
    return spec.fluxDiscrete ? sum : sum / f;
  }

  spawnTick(dt) {
    const done = [];
    for (const e of this.emissions) {
      const before = e.t;
      e.t += dt;
      if (e.t < 0) continue;
      // the frame the emission starts in only counts from its start instant
      const fdt = before < 0 ? e.t : dt;
      const spec = e.spec;
      if (e.burst) {
        // CActionInstanceParticleSpawnerBaseBurst: truncated count, no curve flux,
        // every particle pre-aged by how far into the frame the burst was due
        let m = spec.countDeviation > 0 ? 1 + (this.rng() - 0.5) * spec.countDeviation : 1;
        m *= this._attrFlux(spec);
        const n = Math.trunc(spec.count * m);
        for (let k = 0; k < n; k++) this.spawnFrom(e, n > 1 ? Math.min(k / (n - 1), 1) : 0, k, 0, fdt, 1);
        done.push(e);
        continue;
      }
      // CActionInstanceParticleSpawnerBaseStream
      if (fdt > 0) {
        const cf = this._attrFlux(spec) * this._curveFlux(e, fdt) * e.rate;
        const rem = Math.min(fdt, e.duration - e.age);
        if (cf > 1e-10 && rem > 0) {
          const iv = fdt / cf;
          const emit = (tk) => {
            const lr = isFinite(e.duration) ? (tk + e.age) / e.duration : 0;
            this.spawnFrom(e, lr, e.emitted, tk + e.age, fdt - tk, tk / fdt);
          };
          if (!(spec.countDeviation > 0)) {
            const tot = rem / fdt * (e.acc + cf), n = Math.max(0, Math.floor(tot));
            let t = (1 - e.acc) * iv;
            for (let k = 0; k < n; k++, t += iv) emit(Math.min(t, fdt));
            e.acc = tot - Math.floor(tot);
          } else {
            // count deviation jitters the spacing of each particle
            const d = spec.countDeviation, lo = Math.max(0, 1 - d * 0.5), hi = Math.min(2, 1 + d * 0.5);
            const U = () => lo + (hi - lo) * this.rng();
            let roll = e.roll < 0 ? U() : e.roll;
            let t = roll * iv * (1 - e.acc);
            let guard = 0;
            while (t <= rem && guard++ < 100000) { emit(t); roll = U(); t += roll * iv; }
            e.acc = (rem - (t - roll * iv)) / (roll * iv);
            e.roll = roll;
          }
        }
      }
      e.age += fdt;
      if (e.age >= e.duration) done.push(e);
    }
    if (done.length) this.emissions = this.emissions.filter((e) => !done.includes(e));
  }

  // Fresh particle slot: defaults, bookkeeping, then the FlipBook SetupStream (before
  // the spawn script, so a script that picks TextureID wins).
  _newParticle(lr, sEC, sAge, preAge, grp) {
    const i = this.count;
    this.data.fill(0, i * this.stride, (i + 1) * this.stride);
    this.setAt(i, 'Life', [1]);
    this.setAt(i, 'Color', [1, 1, 1, 1]);
    this.setAt(i, '__rand', [this.rng()]);
    this.setAt(i, '__sLR', [lr]);
    this.setAt(i, '__sEC', [sEC]);
    this.setAt(i, '__sAge', [sAge]);
    this.setAt(i, '__born', [1]);
    this.setAt(i, '__dt', [preAge]);
    this.setAt(i, '__sid', [++this.sys.nextId]);
    this.setAt(i, '__grp', [grp]);
    if (!this._flipbooks) {
      this._flipbooks = []; this._trails = [];
      const walk = (list) => {
        for (const ev of list) {
          if (ev.type === 'flipbook') this._flipbooks.push(ev);
          else if (ev.type === 'spawner') this._trails.push(ev);
          else if (ev.children) walk(ev.children);
        }
      };
      walk(this.L.evolvers);
    }
    for (const ev of this._flipbooks) this.setAt(i, ev.outField, [ev.randomize ? this.rng() * ev.scale + ev.base : ev.base]);
    for (const ev of this._trails) this.setAt(i, ev.accField, [1 - ev.firstDelay]);
    this.count++;
    this._bornPending = true;
    return i;
  }

  // spawn one particle for emission `e`. `lr`/`sEC`/`sAge` are what spawner.* reads,
  // `preAge` is its in-frame dt, `lerpT` where along this frame's emitter motion it left.
  spawnFrom(e, lr, sEC, sAge, preAge, lerpT) {
    if (this.count >= MAX) return;
    const i = this._newParticle(lr, sEC, sAge, preAge, e.id);
    e.emitted++;
    if (this.L.spawnScript) {
      const ctx = this.bindCtx(i, 0, 0);
      ctx._spawnCount = e.spec.count;
      if (e.parentSnap) ctx._parent = { snap: e.parentSnap };
      try { this.L.spawnScript.run(ctx); } catch (err) { warnOnce(this, 'spawn', err); }
      ctx._parent = null;
      if (ctx._dead) { this.count--; return; }
    }
    if (!(this.getAt(i, 'Life')[0] > 0)) { this.count--; return; }
    // place: at the emission origin (event position), else along the emitter's motion
    let base = e.origin;
    if (!base) {
      const m = this.sys.emitter, d = this.sys.emitterDelta, back = e.spec.interpolate ? 1 - lerpT : 0;
      base = [m[0] - d[0] * back, m[1] - d[1] * back, m[2] - d[2] * back];
    }
    if (base[0] || base[1] || base[2]) {
      const p = this.getAt(i, 'Position');
      this.setAt(i, 'Position', [p[0] + base[0], p[1] + base[1], p[2] + base[2]]);
    }
    if (e.vel0) {
      const v = this.getAt(i, 'Velocity');
      this.setAt(i, 'Velocity', [v[0] + e.vel0[0], v[1] + e.vel0[1], v[2] + e.vel0[2]]);
    }
    if (this._needsPrev) this.setAt(i, '__prev', this.getAt(i, 'Position'));
    this.fireEvent('OnSpawn', i);
  }

  // spawn a particle at a parent particle's world position (trail child). The child's
  // spawn script may read parent.<field>; its Position is relative to the parent.
  spawnAt(pos, parentLS, parentIdx, seq, lr, sAge, preAge) {
    if (this.count >= MAX) return;
    const i = this._newParticle(lr, seq, sAge, preAge, parentLS ? parentLS.getAt(parentIdx, '__sid')[0] : 0);
    if (this.L.spawnScript) {
      const ctx = this.bindCtx(i, 0, 0);
      ctx._parent = { ls: parentLS, i: parentIdx };
      try { this.L.spawnScript.run(ctx); } catch (err) { warnOnce(this, 'spawn', err); }
      ctx._parent = null;
      if (ctx._dead) { this.count--; return; }
    }
    if (!(this.getAt(i, 'Life')[0] > 0)) { this.count--; return; }
    const p = this.getAt(i, 'Position');
    this.setAt(i, 'Position', [p[0] + pos[0], p[1] + pos[1], p[2] + pos[2]]);
    if (this.L.inheritVelocity && parentLS) {
      const pv = parentLS.getAt(parentIdx, 'Velocity');
      const v = this.getAt(i, 'Velocity');
      const f = this.L.inheritVelocity;
      this.setAt(i, 'Velocity', [v[0] + pv[0] * f, v[1] + pv[1] * f, v[2] + pv[2] * f]);
    }
    if (this._needsPrev) this.setAt(i, '__prev', this.getAt(i, 'Position'));
    this.fireEvent('OnSpawn', i);
  }

  // Queue this layer's event emissions for particle i (OnSpawn/OnDeath + script events).
  // The parent's fields are snapshotted so child spawn scripts can read parent.<field>
  // even after the parent dies. OnSpawn carries no velocity, so it inherits nothing.
  fireEvent(name, i, at) {
    const targets = this.L.events && this.L.events[name];
    if (!targets) return;
    const pos = at && at.length >= 3 ? at : this.getAt(i, 'Position');
    let snap = null;
    for (const t of targets) {
      const child = this.sys.layers[t.layer];
      if (!child) continue;
      if (!snap) {
        snap = {}; for (const f of this.L.fields) snap[f.name] = this.getAt(i, f.name);
        snap.LifeRatio = [snap.Age[0] / (snap.Life[0] || 1)];
      }
      let vel0 = null;
      if (child.L.inheritVelocity && name !== 'OnSpawn') {
        const pv = snap.Velocity || [0, 0, 0];
        const f = child.L.inheritVelocity;
        vel0 = [pv[0] * f, pv[1] * f, pv[2] * f];
      }
      child.queueEmission(t.spec, pos, vel0, snap);
    }
  }

  // Age + evolve. Newborns use their own in-frame dt; `bornOnly` runs just them.
  update(dt, bornOnly) {
    const L = this.L;
    if (!bornOnly) this._bornPending = false;
    for (let i = 0; i < this.count; i++) {
      const born = this.getAt(i, '__born')[0] > 0;
      if (bornOnly && !born) continue;
      const pdt = born ? this.getAt(i, '__dt')[0] : dt;
      const age = this.getAt(i, 'Age')[0] + pdt;
      const life = this.getAt(i, 'Life')[0] || 1;
      if (age >= life) { this.fireEvent('OnDeath', i); this.kill(i); i--; continue; }
      this.setAt(i, 'Age', [age]);
      const lifeRatio = age / life;
      const ctx = this.bindCtx(i, pdt, lifeRatio);
      this.runEvolvers(L.evolvers, i, pdt, lifeRatio, ctx);
      if (ctx._dead) { this.fireEvent('OnDeath', i); this.kill(i); i--; continue; }
      // safety: cull runaways (data-stiff springs diverge under explicit integration)
      const pp = this.getAt(i, 'Position');
      if (!isFinite(pp[0]) || !isFinite(pp[1]) || !isFinite(pp[2]) ||
          Math.abs(pp[0]) > 1e5 || Math.abs(pp[1]) > 1e5 || Math.abs(pp[2]) > 1e5) { this.kill(i); i--; continue; }
      if (this._needsPrev) this.setAt(i, '__prev', pp);
      if (born) this.setAt(i, '__born', [0]);
    }
  }

  runEvolvers(list, i, dt, lifeRatio, ctx) {
    for (const ev of list) {
      this.runEvolver(ev, i, dt, lifeRatio, ctx);
      if (ctx._dead) return;
    }
  }

  runEvolver(ev, i, dt, lifeRatio, ctx) {
    switch (ev.type) {
      case 'physics': {
        // CParticleKernelCPU_Evolver_Physics. k = Drag * inverse mass; Drag 0 ignores wind.
        // Adaptive strategy: Fast at dt <= IntegrationDtTreshold (0.02), Stable otherwise
        // and always for particles evolved in their spawn frame.
        const v = this.getAt(i, ev.velName), p = this.getAt(i, ev.posField);
        const im = this.field(ev.massField) ? this.getAt(i, ev.massField)[0] : ev.mass;
        let ax = ev.accel[0], ay = ev.accel[1], az = ev.accel[2];
        if (this.field(ev.accelField)) { const q = this.getAt(i, ev.accelField); ax += q[0]; ay += q[1]; az += q[2]; }
        if (this.field(ev.forceField)) { const q = this.getAt(i, ev.forceField); ax += im * q[0]; ay += im * q[1]; az += im * q[2]; }
        const k = ev.drag * im;
        const stable = dt > 0.02 || this.getAt(i, '__born')[0] > 0;
        if (ev.drag === 0 || k === 0) {
          const v0x = v[0], v0y = v[1], v0z = v[2];
          v[0] += ax * dt; v[1] += ay * dt; v[2] += az * dt;
          if (stable) { p[0] += 0.5 * dt * (v0x + v[0]); p[1] += 0.5 * dt * (v0y + v[1]); p[2] += 0.5 * dt * (v0z + v[2]); }
          else { p[0] += v[0] * dt; p[1] += v[1] * dt; p[2] += v[2] * dt; }
        } else {
          let wx = 0, wy = 0, wz = 0;
          if (ev.constVel) { wx = ev.constVel[0]; wy = ev.constVel[1]; wz = ev.constVel[2]; }
          if (this.field(ev.windField)) { const q = this.getAt(i, ev.windField); wx += q[0]; wy += q[1]; wz += q[2]; }
          if (ev.velField) {
            const s = this.L.samplers[ev.velField];
            if (s && s.sampleCurl) { const t = s.sampleCurl(p); wx += t[0]; wy += t[1]; wz += t[2]; }
          }
          const kd = k * dt;
          const e = k > 0.1 ? Math.exp(-kd) : (kd * 0.5 - 1) * kd + 1;
          if (stable) {
            // exact solution of dv/dt = a + k(w - v)
            const c1 = k > 0.1 ? (1 - e) / k : dt - (1 - kd / 3) * dt * 0.5 * kd;
            const c2 = (dt - c1) / k;
            const v0x = v[0], v0y = v[1], v0z = v[2];
            p[0] += c1 * v0x + c2 * ax + k * c2 * wx; p[1] += c1 * v0y + c2 * ay + k * c2 * wy; p[2] += c1 * v0z + c2 * az + k * c2 * wz;
            v[0] = e * v0x + c1 * ax + (1 - e) * wx; v[1] = e * v0y + c1 * ay + (1 - e) * wy; v[2] = e * v0z + c1 * az + (1 - e) * wz;
          } else {
            const c = k > 0.1 ? (1 - e) / k : dt - (1 - kd / 3) * dt * 0.5 * kd;
            const ux = v[0] + ax * dt - wx, uy = v[1] + ay * dt - wy, uz = v[2] + az * dt - wz;
            v[0] = e * ux + wx; v[1] = e * uy + wy; v[2] = e * uz + wz;
            p[0] += c * ux + wx * dt; p[1] += c * uy + wy * dt; p[2] += c * uz + wz * dt;
          }
        }
        this.setAt(i, ev.velName, v); this.setAt(i, ev.posField, p);
        break;
      }
      case 'field': {
        const fi = this.field(ev.field); if (!fi) break;
        const val = ev.curve.sample([lifeRatio]);
        const out = broadcastTo(val, fi.comp);
        this.setAt(i, ev.field, out);
        break;
      }
      case 'rotation': {
        if (!ev.coeff) break;
        let speed;
        if (ev.axial) {
          // spin by how much the axis field points along the view ray
          const cam = this.sys.camPos; if (!cam || !this.field(ev.axialField)) break;
          const p = this.getAt(i, ev.posField), a = this.getAt(i, ev.axialField);
          const dx = p[0] - cam[0], dy = p[1] - cam[1], dz = p[2] - cam[2];
          const l = Math.hypot(dx, dy, dz); if (!(l > 0)) break;
          speed = (dx * a[0] + dy * a[1] + dz * a[2]) / l;
        } else {
          if (!this.field(ev.speedField)) break;
          speed = this.getAt(i, ev.speedField)[0];
        }
        if (speed) {
          const r = this.getAt(i, ev.angleField); r[0] += speed * ev.coeff * dt;
          this.setAt(i, ev.angleField, r);
        }
        break;
      }
      case 'damper': {
        // CParticleKernelCPU_Evolver_Damper: v *= exp(-rate*dt), then lift to MinSpeed
        const fi = this.field(ev.field); if (!fi) break;
        const v = this.getAt(i, ev.field);
        const f = Math.exp(-ev.rate * dt), ms = ev.minSpeed;
        if (!ms) { this.setAt(i, ev.field, v.map((x) => x * f)); break; }
        if (fi.comp === 1) { const y = v[0] * f; this.setAt(i, ev.field, [Math.abs(y) >= ms ? y : (v[0] < 0 ? -ms : ms)]); break; }
        let m2 = 0; for (const x of v) m2 += x * x;
        if (m2 * f * f > ms * ms) { this.setAt(i, ev.field, v.map((x) => x * f)); break; }
        if (m2 <= 1e-10) { const o = new Array(v.length).fill(0); o[0] = ms; this.setAt(i, ev.field, o); break; }
        const sc = ms / Math.sqrt(m2);
        this.setAt(i, ev.field, v.map((x) => x * sc));
        break;
      }
      case 'flipbook': {
        // frame = cursor*scale + base, looped via frac when LoopCount != 1
        if (!ev.cursorOk) break;
        const c = ev.cursor === 'LifeRatio' ? lifeRatio : this.getAt(i, ev.cursor)[0];
        let x = c;
        if (ev.loop !== 1) { x = c * ev.loop; x -= Math.trunc(x); }
        this.setAt(i, ev.outField, [x * ev.scale + ev.base]);
        break;
      }
      case 'spawner': {
        // trail (CParticleKernelCPU_Evolver_Spawner): children spaced evenly along this
        // frame's path, each pre-aged by the rest of the frame after its instant
        let m;
        if (ev.metric === 'Time') m = dt;
        else if (ev.metric === 'Distance') {
          const p = this.getAt(i, 'Position'), q = this.getAt(i, '__prev');
          m = Math.hypot(p[0] - q[0], p[1] - q[1], p[2] - q[2]);
        } else break;
        let flux = 1;
        if (ev.flux) {
          const x = ev.tile * lifeRatio;
          flux = ev.flux.sample([ev.tile <= 1 ? Math.min(x, 1) : x - Math.floor(x)])[0];
          if (!(flux >= 1e-8)) break;
        }
        const count = flux / ev.interval * m;
        if (!(count > 1e-10 && count < 1e5)) break;
        const acc0 = this.getAt(i, ev.accField)[0], tot = acc0 + count, n = Math.floor(tot);
        this.setAt(i, ev.accField, [tot - n]);
        if (!n) break;
        const child = this.sys.layers[ev.child];
        if (!child) break;
        const p = this.getAt(i, 'Position'), q = this.getAt(i, '__prev');
        const life = this.getAt(i, 'Life')[0] || 1, age0 = this.getAt(i, 'Age')[0] - dt;
        let sEC = this.getAt(i, ev.countField)[0];
        for (let k = 0; k < n; k++) {
          const f = (k + 1 - acc0) / count, tk = Math.min(f * dt, dt);
          const pos = [q[0] + (p[0] - q[0]) * f, q[1] + (p[1] - q[1]) * f, q[2] + (p[2] - q[2]) * f];
          child.spawnAt(pos, this, i, sEC++, (age0 + tk) / life, age0 + tk, dt - tk);
        }
        this.setAt(i, ev.countField, [sEC]);
        break;
      }
      case 'localspace': {
        // apply the emitter's per-frame movement delta to transform-filtered fields
        // (Position + custom full/translate fields) -> attached to the emitter
        // newborns enter with the Current transform, so they take no delta
        if (ev.attach && !this.getAt(i, '__born')[0]) {
          const d = this.sys.emitterDelta;
          if (d[0] || d[1] || d[2]) {
            const s = ev.attach;
            const p = this.getAt(i, 'Position');
            this.setAt(i, 'Position', [p[0] + d[0] * s, p[1] + d[1] * s, p[2] + d[2] * s]);
            for (const f of this.L.fields) {
              const fi = this.L.fieldIndex[f.name];
              if (fi.tf === 'full' || fi.tf === 'translate') {
                const v = this.getAt(i, f.name);
                this.setAt(i, f.name, [(v[0] || 0) + d[0] * s, (v[1] || 0) + d[1] * s, (v[2] || 0) + d[2] * s]);
              }
            }
          }
        }
        if (ev.children.length) this.runEvolvers(ev.children, i, dt, lifeRatio, ctx);
        break;
      }
      case 'attractor': {
        // CParticleKernelCPU_Evolver_Attractor: Force toward the shape SURFACE, signed
        // distance d (negative inside), Physical (inverse square) or Finite falloff
        const s = ev.shape ? this.L.samplers[ev.shape] : null;
        if (!s || !s.project) break;
        const pr = s.project(this.getAt(i, ev.posField));
        if (!pr) break;
        const [vx, vy, vz, d] = pr;
        const ad = Math.abs(d);
        let g = 0;
        if (ad > 1e-10) {
          const ds = ev.repulse ? ad : d;
          if (ev.finite) {
            const st = Math.max(ev.steepness, 0.001), R = Math.max(ev.influence, 1e-6);
            const c = (1 - st * st) / (1 + st * st), q = d * d * (st / R) * (st / R);
            g = ev.force * Math.max(0, ((1 - q) / (1 + q) - c) / (1 - c)) / ds;
          } else {
            g = ev.force / ds / ((ad + 1) * (ad + 1));
          }
        }
        this.setAt(i, ev.forceField, [vx * g, vy * g, vz * g]);
        break;
      }
      case 'script': {
        if (ev.disabled) break;
        try { ev.script.run(ctx); }
        catch (err) {
          if (++ev.errors >= 3) ev.disabled = true;
          warnOnce(this, 'evolve', err);
        }
        break;
      }
      // 'unsupported' -> no-op
    }
  }

  kill(i) {
    const last = this.count - 1;
    if (i !== last) {
      const a = i * this.stride, b = last * this.stride;
      this.data.copyWithin(a, b, b + this.stride);
    }
    this.count--;
  }

  // ---- field accessors ----
  getAt(i, name) {
    const fi = this.L.fieldIndex[name]; if (!fi) return [0];
    const base = i * this.stride + fi.offset; const o = new Array(fi.comp);
    for (let k = 0; k < fi.comp; k++) o[k] = this.data[base + k];
    return o;
  }
  setAt(i, name, val) {
    const fi = this.L.fieldIndex[name]; if (!fi) return;
    const base = i * this.stride + fi.offset;
    for (let k = 0; k < fi.comp; k++) this.data[base + k] = val[k] ?? val[0] ?? 0;
  }

  // ---- script context ----
  makeCtx() {
    const self = this;
    return {
      _i: 0, _dt: 0, _lr: 0, _dead: false, _parent: null, _spawnCount: 0,
      getField(name) {
        if (name === 'LifeRatio') return [self._ctx._lr];
        if (name === 'dt') return [self._ctx._dt];
        const fi = self.L.fieldIndex[name]; if (!fi) return null;
        return self.getAt(self._ctx._i, name);
      },
      setField(name, val) { self.setAt(self._ctx._i, name, val); },
      hasField(name) { return !!self.L.fieldIndex[name]; },
      sampler(name) { return self.L.samplers[name] || null; },
      attribute(name) { return (self.attributes && self.attributes[name]) || null; },
      parentField(name) {
        const p = self._ctx._parent;
        if (!p) return [0, 0, 0];
        if (p.snap) return p.snap[name] || [0, 0, 0];
        if (name === 'LifeRatio') return [p.ls.getAt(p.i, 'Age')[0] / (p.ls.getAt(p.i, 'Life')[0] || 1)];
        return p.ls.getAt(p.i, name);
      },
      spawnerField(name) {
        const c = self._ctx;
        if (name === 'LifeRatio') return self.getAt(c._i, '__sLR');
        if (name === 'EmittedCount') return self.getAt(c._i, '__sEC');
        if (name === 'Age') return self.getAt(c._i, '__sAge');
        if (name === 'SpawnCount') return [c._spawnCount || (self.L.spawn ? self.L.spawn.count : 0)];
        return [0];
      },
      // EventName.trigger(cond[, position, axis1, axis2]): queue the event's child emission
      triggerEvent(name, args) {
        const cond = args && args.length ? args[0][0] : 1;
        if (cond) self.fireEvent(name, self._ctx._i, args && args.length >= 2 ? args[1] : null);
      },
      rand(a, b) { return a + self.rng() * (b - a); },
      // vrand(a, b): uniform direction, radius between min and max (so vrand(-k, k) is ON
      // the sphere of radius k, vrand(0, k) fills the ball)
      vrand(a, b) {
        const lo = Math.min(a, b), hi = Math.max(a, b);
        const p = hi === 0 ? 0 : (lo / hi + 1) / 3;
        const r = lo + (hi - lo) * Math.pow(self.rng(), p);
        const y = r - 2 * self.rng() * r, rho = Math.sqrt(Math.max(r * r - y * y, 0)), t = 2 * Math.PI * self.rng();
        return [rho * Math.cos(t), y, rho * Math.sin(t)];
      },
      sceneField(name) { return name === 'Time' ? [self.sys.clock] : [0]; },
      kill() { self._ctx._dead = true; },
      trigger() {},
    };
  }
  bindCtx(i, dt, lifeRatio) { const c = this._ctx; c._i = i; c._dt = dt; c._lr = lifeRatio; c._dead = false; c._parent = null; return c; }
}

function warnOnce(ls, phase, err) {
  if (ls._warned) return;
  ls._warned = true;
  console.warn(`pkfx ${phase} script error in layer ${ls.L.name}:`, err.message);
}

function broadcastTo(v, comp) {
  if (v.length === comp) return v;
  if (v.length === 1) return new Array(comp).fill(v[0]);
  const o = new Array(comp); for (let k = 0; k < comp; k++) o[k] = v[k] ?? v[v.length - 1] ?? 0; return o;
}

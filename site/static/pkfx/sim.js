// CPU particle simulation. One LayerSim per Layer holds a struct-of-arrays buffer
// (`data`, stride = sum of field components). Alive particles occupy [0, count);
// death is a swap-remove. Each frame: process emissions (spawn), then age + run
// the evolver pipeline.
//
// Emissions are the spawn engine: a root layer gets one emission per loop, and
// events (OnDeath, script triggers) queue emissions at a world position. Each
// emission is rate-driven (particles/second by default; TotalParticleCount over
// DurationInSeconds when written), can be infinite, pulse-burst (ContinuousSpawner
// = false), and flux-modulated by an attribute and/or a curve.
import { TurbulenceSampler } from './curves.js';

const MAX = 20000;      // per-layer particle cap
const MAX_EMISSIONS = 128;

export class System {
  constructor(effect, rng = Math.random) {
    this.effect = effect;
    this.rng = rng;
    // World offset applied to root spawns. The preview moves this in a slow orbit for
    // trail effects; localspace-attached layers follow it via emitterDelta.
    this.emitter = new Float32Array(3);
    this._emitterPrev = new Float32Array(3);
    this.emitterDelta = new Float32Array(3);
    this.attributes = effect.attributes || {};
    this.layers = effect.layers.map((l) => { const ls = new LayerSim(l, rng); ls.sys = this; ls.attributes = this.attributes; return ls; });
    // every turbulence sampler needs the running time for animation
    this._turb = [];
    for (const ls of this.layers) for (const s of Object.values(ls.L.samplers)) if (s instanceof TurbulenceSampler) this._turb.push(s);
    this.time = 0;
    this.reset();
  }

  reset() {
    this.time = 0;
    // weighted pick per random-child group: only the chosen alternative spawns
    const picks = new Map();
    for (const l of this.layers) {
      const g = l.L.spawn && l.L.spawn.group;
      if (!g || picks.has(g.id)) continue;
      const members = this.layers.filter((o) => o.L.spawn && o.L.spawn.group && o.L.spawn.group.id === g.id);
      let total = 0; for (const m of members) total += m.L.spawn.group.weight || 1;
      let r = this.rng() * (total || 1);
      let chosen = members[0];
      for (const m of members) { r -= m.L.spawn.group.weight || 1; if (r <= 0) { chosen = m; break; } }
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
    this.emitterDelta[0] = this.emitter[0] - this._emitterPrev[0];
    this.emitterDelta[1] = this.emitter[1] - this._emitterPrev[1];
    this.emitterDelta[2] = this.emitter[2] - this._emitterPrev[2];
    this._emitterPrev.set(this.emitter);
    for (const t of this._turb) t.time = this.time;

    let anyAlive = false, anyPending = false;
    for (const l of this.layers) {
      l.spawnTick(dt);
      l.update(dt);
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
    this._ctx = this.makeCtx();
  }
  clear() { this.count = 0; this.emissions = []; }

  field(name) { return this.L.fieldIndex[name]; }

  // Activate an emission of `spec` at world `origin` (null -> the moving emitter),
  // optionally seeding spawned particles with an inherited velocity.
  queueEmission(spec, origin, vel0, parentSnap) {
    if (this.emissions.length >= MAX_EMISSIONS) return;
    const dev = spec.countDeviation ? Math.max(0, 1 + (this.rng() * 2 - 1) * spec.countDeviation) : 1;
    const ddev = spec.durationDeviation ? Math.max(0.05, 1 + (this.rng() * 2 - 1) * spec.durationDeviation) : 1;
    this.emissions.push({
      spec,
      count: spec.count * dev,
      duration: Math.max(spec.duration * ddev, 0),
      t: -(spec.delay + (spec.randomDelay ? this.rng() * spec.randomDelay : 0) + spec.firstDelay),
      acc: 0, bursts: 0, emitted: 0,
      origin: origin ? [origin[0], origin[1], origin[2]] : null,
      vel0: vel0 || null,
      parentSnap: parentSnap || null,
    });
  }

  _flux(e) {
    const spec = e.spec;
    let f = 1;
    if (spec.fluxAttr) {
      const a = this.attributes && this.attributes[spec.fluxAttr];
      // a declared default of 0 means "off until the game drives it" — preview at 1
      if (a && a[0]) f *= a[0];
    }
    if (spec.fluxCurve) {
      const tile = spec.fluxTile > 0 ? spec.fluxTile : (e.duration || 1);
      const x = tile > 0 ? (e.t / tile) % 1 : 0;
      f *= spec.fluxCurve.sample([x < 0 ? x + 1 : x])[0] ?? 1;
    }
    return Math.max(0, f);
  }

  spawnTick(dt) {
    const done = [];
    for (const e of this.emissions) {
      e.t += dt;
      if (e.t < 0) continue;
      const spec = e.spec;
      const dur = e.duration;
      const flux = this._flux(e);
      if (!spec.continuous) {
        // pulse: the full count at every period boundary (once if not infinite)
        const period = Math.max(dur, 1e-3);
        const due = Math.floor(e.t / period) + 1;
        const per = Math.max(1, Math.round((spec.totalMode ? e.count : e.count * period) * flux));
        while (e.bursts < due && (spec.infinite || e.bursts < 1)) {
          for (let k = 0; k < per; k++) this.spawnFrom(e, per > 1 ? k / (per - 1) : 0);
          e.bursts++;
        }
        if (!spec.infinite && e.bursts >= 1) done.push(e);
        continue;
      }
      if (dur <= 1e-6 && spec.totalMode) {
        // instantaneous burst
        const n = Math.max(1, Math.round(e.count * flux));
        for (let k = 0; k < n; k++) this.spawnFrom(e, n > 1 ? k / (n - 1) : 0);
        done.push(e);
        continue;
      }
      // continuous emission: the count deviation re-rolls over time (rate jitter),
      // so a low roll makes sparse puffs rather than permanently killing the layer
      const cnt = spec.countDeviation
        ? spec.count * Math.max(0, 1 + (this.rng() * 2 - 1) * spec.countDeviation)
        : e.count;
      const rate = spec.totalMode ? cnt / Math.max(dur, 1e-3) : cnt;
      const over = !spec.infinite && e.t >= dur;
      if (!over) {
        e.acc += rate * flux * dt;
        let n = Math.floor(e.acc);
        e.acc -= n;
        const lrBase = dur > 0 ? (e.t / dur) % 1 : 0;
        while (n-- > 0) this.spawnFrom(e, lrBase);
      } else {
        done.push(e);
      }
    }
    if (done.length) this.emissions = this.emissions.filter((e) => !done.includes(e));
  }

  // spawn one particle for emission `e`; `lr` is the spawner LifeRatio at this moment
  spawnFrom(e, lr) {
    if (this.count >= MAX) return;
    const i = this.count;
    this.data.fill(0, i * this.stride, (i + 1) * this.stride);
    this.setAt(i, 'Life', [1]);
    this.setAt(i, 'Color', [1, 1, 1, 1]);
    this.setAt(i, '__rand', [this.rng()]);
    this.setAt(i, '__sLR', [lr]);
    this.setAt(i, '__sEC', [e.emitted]);
    this.setAt(i, '__sAge', [Math.max(e.t, 0)]);
    this.count++;
    e.emitted++;
    if (this.L.spawnScript) {
      const ctx = this.bindCtx(i, 0, 0);
      ctx._spawnCount = e.count;
      if (e.parentSnap) ctx._parent = { snap: e.parentSnap };
      try { this.L.spawnScript.run(ctx); } catch (err) { warnOnce(this, 'spawn', err); }
      ctx._parent = null;
      if (ctx._dead) { this.count--; return; }
    }
    if (!(this.getAt(i, 'Life')[0] > 0)) { this.count--; return; }
    // place: at the emission origin (event position), else at the moving emitter
    const base = e.origin || this.sys.emitter;
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
  spawnAt(pos, parentLS, parentIdx, seq, lr) {
    if (this.count >= MAX) return;
    const i = this.count;
    this.data.fill(0, i * this.stride, (i + 1) * this.stride);
    this.setAt(i, 'Life', [1]);
    this.setAt(i, 'Color', [1, 1, 1, 1]);
    this.setAt(i, '__rand', [this.rng()]);
    this.setAt(i, '__sLR', [lr || 0]);
    this.setAt(i, '__sEC', [seq || 0]);
    this.count++;
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
  // even after the parent dies.
  fireEvent(name, i) {
    const targets = this.L.events && this.L.events[name];
    if (!targets) return;
    const pos = this.getAt(i, 'Position');
    let snap = null;
    for (const t of targets) {
      const child = this.sys.layers[t.layer];
      if (!child) continue;
      if (!snap) { snap = {}; for (const f of this.L.fields) snap[f.name] = this.getAt(i, f.name); }
      let vel0 = null;
      if (child.L.inheritVelocity) {
        const pv = snap.Velocity || [0, 0, 0];
        const f = child.L.inheritVelocity;
        vel0 = [pv[0] * f, pv[1] * f, pv[2] * f];
      }
      child.queueEmission(t.spec, pos, vel0, snap);
    }
  }

  update(dt) {
    const L = this.L;
    for (let i = 0; i < this.count; i++) {
      const age = this.getAt(i, 'Age')[0] + dt;
      const life = this.getAt(i, 'Life')[0] || 1;
      if (age >= life) { this.fireEvent('OnDeath', i); this.kill(i); i--; continue; }
      this.setAt(i, 'Age', [age]);
      const lifeRatio = age / life;
      const ctx = this.bindCtx(i, dt, lifeRatio);
      this.runEvolvers(L.evolvers, i, dt, lifeRatio, ctx);
      if (ctx._dead) { this.fireEvent('OnDeath', i); this.kill(i); i--; continue; }
      // safety: cull runaways (data-stiff springs diverge under explicit integration)
      const pp = this.getAt(i, 'Position');
      if (!isFinite(pp[0]) || !isFinite(pp[1]) || !isFinite(pp[2]) ||
          Math.abs(pp[0]) > 1e5 || Math.abs(pp[1]) > 1e5 || Math.abs(pp[2]) > 1e5) { this.kill(i); i--; continue; }
      if (this._needsPrev) this.setAt(i, '__prev', pp);
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
        const v = this.getAt(i, ev.velName); const p = this.getAt(i, ev.posField);
        v[0] += ev.accel[0] * dt; v[1] += ev.accel[1] * dt; v[2] += ev.accel[2] * dt;
        if (ev.drag) {
          // drag pulls the velocity toward the medium's velocity field (wind/turbulence)
          let ux = 0, uy = 0, uz = 0;
          if (ev.constVel) { ux = ev.constVel[0]; uy = ev.constVel[1]; uz = ev.constVel[2]; }
          if (ev.velField) {
            const s = this.L.samplers[ev.velField];
            if (s && s.sampleCurl) { const t = s.sampleCurl(p); ux += t[0]; uy += t[1]; uz += t[2]; }
          }
          const f = Math.exp(-ev.drag * dt / ev.mass);
          v[0] = ux + (v[0] - ux) * f; v[1] = uy + (v[1] - uy) * f; v[2] = uz + (v[2] - uz) * f;
        } else if (ev.velField || ev.constVel) {
          // no drag: the field velocity advects the particle directly
          let ux = 0, uy = 0, uz = 0;
          if (ev.constVel) { ux = ev.constVel[0]; uy = ev.constVel[1]; uz = ev.constVel[2]; }
          if (ev.velField) {
            const s = this.L.samplers[ev.velField];
            if (s && s.sampleCurl) { const t = s.sampleCurl(p); ux += t[0]; uy += t[1]; uz += t[2]; }
          }
          p[0] += ux * dt; p[1] += uy * dt; p[2] += uz * dt;
        }
        p[0] += v[0] * dt; p[1] += v[1] * dt; p[2] += v[2] * dt;
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
        const sf = this.field(ev.speedField); if (!sf) break;
        const speed = this.getAt(i, ev.speedField)[0];
        if (speed) {
          const r = this.getAt(i, ev.angleField); r[0] += speed * dt;
          this.setAt(i, ev.angleField, r);
        }
        break;
      }
      case 'damper': {
        const fi = this.field(ev.field); if (!fi) break;
        const v = this.getAt(i, ev.field);
        let f = Math.exp(-dt / Math.max(ev.time, 1e-3));
        if (ev.minSpeed > 0) {
          let m = 0; for (const x of v) m += x * x; m = Math.sqrt(m);
          if (m > 1e-9) f = Math.max(f, Math.min(ev.minSpeed, m) / m);
        }
        this.setAt(i, ev.field, v.map((x) => x * f));
        break;
      }
      case 'flipbook': {
        const fi = this.field(ev.outField); if (!fi) break;
        const frames = Math.max(1, ev.last - ev.first + 1);
        let cursor = ev.cursorField ? (this.getAt(i, ev.cursorField)[0] || 0) : lifeRatio;
        let pos = (cursor * ev.loop) % 1; if (pos < 0) pos += 1;
        let frame = pos * frames;
        if (ev.randomize) frame += this.getAt(i, '__rand')[0] * frames;
        this.setAt(i, ev.outField, [ev.first + (frame % frames)]);
        break;
      }
      case 'spawner': {
        // trail: emit children into the child layer along this particle's path
        const age = this.getAt(i, 'Age')[0];
        if (age < ev.firstDelay) break;
        let n = 0;
        if (ev.metric === 'Time') {
          n = Math.floor((age - ev.firstDelay) / ev.interval) - Math.floor((age - dt - ev.firstDelay) / ev.interval);
        } else {
          // Distance (the default): accumulate movement since last frame
          const p = this.getAt(i, 'Position'); const q = this.getAt(i, '__prev');
          const d = Math.hypot(p[0] - q[0], p[1] - q[1], p[2] - q[2]);
          let acc = this.getAt(i, ev.accField)[0] + d;
          n = Math.floor(acc / ev.interval);
          this.setAt(i, ev.accField, [acc - n * ev.interval]);
        }
        if (n > 0) {
          n = Math.min(n, 8);
          const pos = this.getAt(i, 'Position');
          const child = this.sys.layers[ev.child];
          if (child) for (let k = 0; k < n; k++) child.spawnAt(pos, this, i, k, lifeRatio);
        }
        break;
      }
      case 'localspace': {
        // apply the emitter's per-frame movement delta to transform-filtered fields
        // (Position + custom full/translate fields) -> attached to the emitter
        if (ev.attach) {
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
        const s = ev.shape ? this.L.samplers[ev.shape] : null;
        const target = s && s.pos ? s.pos : [0, 0, 0];
        const p = this.getAt(i, 'Position');
        let dx = target[0] - p[0], dy = target[1] - p[1], dz = target[2] - p[2];
        const d = Math.hypot(dx, dy, dz);
        if (d < 1e-6) break;
        if (ev.influence > 0 && d > ev.influence) break;
        const f = (ev.repulse ? -1 : 1) * ev.force * dt / d;
        const v = this.getAt(i, 'Velocity');
        this.setAt(i, 'Velocity', [v[0] + dx * f, v[1] + dy * f, v[2] + dz * f]);
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
      // EventName.trigger(cond): queue the event's child emission at this particle
      triggerEvent(name, args) {
        const cond = args && args.length ? args[0][0] : 1;
        if (cond) self.fireEvent(name, self._ctx._i);
      },
      rand(a, b) { return a + self.rng() * (b - a); },
      vrand(a, b) { return [a + self.rng() * (b - a), a + self.rng() * (b - a), a + self.rng() * (b - a)]; },
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

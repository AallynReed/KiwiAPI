// CPU particle simulation. One LayerSim per Layer holds a struct-of-arrays buffer
// (`data`, stride = sum of field components). Alive particles occupy [0, count);
// death is a swap-remove. Each frame: spawn, then age + run the evolver pipeline.
import { vmul } from './script.js';

const MAX = 20000; // per-layer particle cap

export class System {
  constructor(effect, rng = Math.random) {
    this.effect = effect;
    this.rng = rng;
    // World offset applied to each particle's spawn position. The preview moves this
    // in a slow orbit for trail (ribbon) effects so the ribbon has a path to follow;
    // it stays [0,0,0] for everything else.
    this.emitter = new Float32Array(3);
    this.attributes = effect.attributes || {};
    this.layers = effect.layers.map((l) => { const ls = new LayerSim(l, rng); ls._emitter = this.emitter; ls.sys = this; ls.attributes = this.attributes; return ls; });
    this.time = 0;
    this.started = this.layers.map(() => false);
  }

  // spawn one particle into a child layer at a parent particle's position (the real trigger
  // mechanism: trail spawner-evolvers + script events emit children from their parent).
  spawnChild(childIdx, pos, parentLS, parentIdx) {
    const child = this.layers[childIdx];
    if (child) child.spawnAt(pos, parentLS, parentIdx);
  }
  reset() { for (const l of this.layers) l.clear(); this.time = 0; this.started = this.layers.map(() => false); }

  // advance the whole effect; auto-loops when everything is finished and empty
  update(dt) {
    this.time += dt;
    let anyAlive = false, anyEmitting = false;
    for (let i = 0; i < this.layers.length; i++) {
      const l = this.layers[i];
      l.spawnTick(this.time, dt);
      l.update(dt);
      if (l.count > 0) anyAlive = true;
      if (!l.L.isChild && !l.spawnDone) anyEmitting = true;   // only root layers emit on their own
    }
    if (!anyAlive && !anyEmitting) this.reset();
  }
}

class LayerSim {
  constructor(layer, rng) {
    this.L = layer; this.rng = rng;
    this.stride = layer.stride;
    this.data = new Float32Array(MAX * this.stride);
    this.count = 0;
    this.spawnAccum = 0;
    this.spawnedBurst = false;
    this.spawnDone = false;
    this.startTime = layer.delay;
    this._ctx = this.makeCtx();
  }
  clear() { this.count = 0; this.spawnAccum = 0; this.spawnedBurst = false; this.spawnDone = false; }

  field(name) { return this.L.fieldIndex[name]; }

  spawnTick(time, dt) {
    const L = this.L;
    if (L.isChild) return;        // child layers spawn only from a parent particle
    if (time < L.delay) return;
    if (L.duration > 0) {
      if (this.spawnDone) return;
      const rate = L.spawnCount / L.duration;
      this.spawnAccum += rate * dt;
      let toSpawn = Math.floor(this.spawnAccum);
      this.spawnAccum -= toSpawn;
      while (toSpawn-- > 0) this.spawn();
      if (time - L.delay >= L.duration && !L.infinite) this.spawnDone = true;
    } else {
      // burst at activation
      if (!this.spawnedBurst) {
        const n = Math.max(1, Math.round(L.spawnCount || 1));
        for (let k = 0; k < n; k++) this.spawn();
        this.spawnedBurst = true;
        this.spawnDone = true;
      }
    }
  }

  spawn() {
    if (this.count >= MAX) return;
    const i = this.count;
    const base = i * this.stride;
    this.data.fill(0, base, base + this.stride);
    // default Life so a particle without an explicit Life still lives a frame
    this.setAt(i, 'Life', [1]);
    this.setAt(i, 'Color', [1, 1, 1, 1]);
    this.count++;
    // run spawn script
    if (this.L.spawnScript) {
      const ctx = this.bindCtx(i, 0, 0);
      this.L.spawnScript.run(ctx);
      if (ctx._dead) { this.count--; return; }
    }
    const life = this.getAt(i, 'Life')[0];
    if (!(life > 0)) { this.count--; return; }
    // offset the spawn position by the (possibly moving) emitter
    const e = this._emitter;
    if (e && (e[0] || e[1] || e[2])) {
      const p = this.getAt(i, 'Position');
      this.setAt(i, 'Position', [p[0] + e[0], p[1] + e[1], p[2] + e[2]]);
    }
    this.fireEvent('OnSpawn', i);
  }

  // spawn a particle at a parent particle's world position (trail/event child). The child's
  // spawn script may read parent.<field> and sets Position RELATIVE to the parent.
  spawnAt(pos, parentLS, parentIdx) {
    if (this.count >= MAX) return;
    const i = this.count;
    const base = i * this.stride;
    this.data.fill(0, base, base + this.stride);
    this.setAt(i, 'Life', [1]);
    this.setAt(i, 'Color', [1, 1, 1, 1]);
    this.count++;
    if (this.L.spawnScript) {
      const ctx = this.bindCtx(i, 0, 0);
      ctx._parent = { ls: parentLS, i: parentIdx };
      this.L.spawnScript.run(ctx);
      ctx._parent = null;
      if (ctx._dead) { this.count--; return; }
    }
    const p = this.getAt(i, 'Position');     // pos already includes the emitter (parent had it)
    this.setAt(i, 'Position', [p[0] + pos[0], p[1] + pos[1], p[2] + pos[2]]);
    if (!(this.getAt(i, 'Life')[0] > 0)) { this.count--; return; }
    this.fireEvent('OnSpawn', i);
  }

  // Spawn this layer's children for a named event at particle i's position. Used for the
  // built-in lifecycle events (OnSpawn at birth, OnDeath at death) and script triggers.
  fireEvent(name, i) {
    const targets = this.L.events && this.L.events[name];
    if (!targets) return;
    const pos = this.getAt(i, 'Position');
    for (const t of targets) for (let k = 0; k < t.count; k++) this.sys.spawnChild(t.layer, pos, this, i);
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
      for (const ev of L.evolvers) {
        this.runEvolver(ev, i, dt, lifeRatio, ctx);
        if (ctx._dead) break;
      }
      if (ctx._dead) { this.fireEvent('OnDeath', i); this.kill(i); i--; }
    }
  }

  runEvolver(ev, i, dt, lifeRatio, ctx) {
    switch (ev.type) {
      case 'physics': {
        const v = this.getAt(i, 'Velocity'); const p = this.getAt(i, 'Position');
        v[0] += ev.accel[0] * dt; v[1] += ev.accel[1] * dt; v[2] += ev.accel[2] * dt;
        if (ev.drag) { const f = Math.max(0, 1 - ev.drag * dt); v[0] *= f; v[1] *= f; v[2] *= f; }
        p[0] += v[0] * dt; p[1] += v[1] * dt; p[2] += v[2] * dt;
        this.setAt(i, 'Velocity', v); this.setAt(i, 'Position', p);
        break;
      }
      case 'field': {
        const fi = this.field(ev.field); if (!fi) break;
        const val = ev.curve.sample([lifeRatio]);
        const out = broadcastTo(val, fi.comp);
        this.setAt(i, ev.field, out);
        break;
      }
      case 'rotation': { const r = this.getAt(i, 'Rotation'); r[0] += ev.speed * dt; this.setAt(i, 'Rotation', r); break; }
      case 'damper': {
        const fi = this.field(ev.field); if (!fi) break;
        const v = this.getAt(i, ev.field); const f = Math.exp(-dt / (ev.time || 0.1));
        this.setAt(i, ev.field, v.map((x) => x * f));
        break;
      }
      case 'flipbook': {
        const fi = this.field('TextureID'); if (!fi) break;
        this.setAt(i, 'TextureID', [lifeRatio * (ev.lastFrame + 1)]);
        break;
      }
      case 'spawner': {
        // trail: emit children into the child layer over time (Age-based; covers the Time
        // metric and approximates Distance) at this particle's current position
        const interval = ev.interval > 1e-4 ? ev.interval : 0.05;
        const age = this.getAt(i, 'Age')[0];
        let n = Math.floor(age / interval) - Math.floor((age - dt) / interval);
        if (n > 0) {
          n = Math.min(n, 8);
          const pos = this.getAt(i, 'Position');
          for (let k = 0; k < n; k++) this.sys.spawnChild(ev.child, pos, this, i);
        }
        break;
      }
      case 'script': ev.script.run(ctx); break;
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
      _i: 0, _dt: 0, _lr: 0, _dead: false, _parent: null,
      getField(name) {
        if (name === 'LifeRatio') return [self._ctx._lr];
        const fi = self.L.fieldIndex[name]; if (!fi) return null;
        return self.getAt(self._ctx._i, name);
      },
      setField(name, val) { self.setAt(self._ctx._i, name, val); },
      hasField(name) { return !!self.L.fieldIndex[name]; },
      sampler(name) { return self.L.samplers[name] || null; },
      attribute(name) { return (self.attributes && self.attributes[name]) || null; },
      parentField(name) { const p = self._ctx._parent; return p ? p.ls.getAt(p.i, name) : [0]; },
      // EventName.trigger(cond): fire the script event (spawn its child layer at this
      // particle) when cond is truthy — the real trigger, not a no-op.
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

function broadcastTo(v, comp) {
  if (v.length === comp) return v;
  if (v.length === 1) return new Array(comp).fill(v[0]);
  const o = new Array(comp); for (let k = 0; k < comp; k++) o[k] = v[k] ?? v[v.length - 1] ?? 0; return o;
}

// Compiler + interpreter for the PopcornFX v1.x particle script language
// (the body of CCompilerSyntaxNodeExpression `function void Eval() { ... }`).
//
// It is a small HLSL-like language: scalar/vector (1..4 component) values, field
// reads/writes, local variable declarations, sampler method calls, and a library
// of intrinsics. We tokenize -> parse to an AST -> interpret against a runtime
// context (one particle at a time). Values are plain JS number arrays; a scalar
// is a length-1 array. Arithmetic broadcasts scalar<->vector componentwise.
// Programs may define helper functions (Run, PostEval, ...) besides Eval; the
// entry point is Eval when present.

const PI = Math.PI;

// ---------------- Values ----------------
const S = (x) => [x];
const dim = (v) => v.length;

function broadcast(a, b) {
  // returns [aExpanded, bExpanded, n]
  const na = a.length, nb = b.length;
  if (na === nb) return [a, b, na];
  if (na === 1) { const n = nb; const ax = new Array(n).fill(a[0]); return [ax, b, n]; }
  if (nb === 1) { const n = na; const bx = new Array(n).fill(b[0]); return [a, bx, n]; }
  // mismatched non-scalar: operate on the smaller length
  const n = Math.min(na, nb);
  return [a.slice(0, n), b.slice(0, n), n];
}
function ew(a, b, f) { const [x, y, n] = broadcast(a, b); const o = new Array(n); for (let i = 0; i < n; i++) o[i] = f(x[i], y[i]); return o; }
const vadd = (a, b) => ew(a, b, (x, y) => x + y);
const vsub = (a, b) => ew(a, b, (x, y) => x - y);
const vmul = (a, b) => ew(a, b, (x, y) => x * y);
const vdiv = (a, b) => ew(a, b, (x, y) => x / y);
const vneg = (a) => a.map((x) => -x);

// ---------------- Tokenizer ----------------
const TYPE_KEYWORDS = new Set(['float', 'float2', 'float3', 'float4', 'int', 'int2', 'int3', 'int4', 'bool', 'void']);

function tokenize(src) {
  const toks = [];
  let i = 0; const n = src.length;
  const push = (t, v) => toks.push({ t, v });
  while (i < n) {
    const c = src[i];
    if (c === ' ' || c === '\t' || c === '\r' || c === '\n') { i++; continue; }
    if (c === '/' && src[i + 1] === '/') { while (i < n && src[i] !== '\n') i++; continue; }
    if (c === '/' && src[i + 1] === '*') { i += 2; while (i < n && !(src[i] === '*' && src[i + 1] === '/')) i++; i += 2; continue; }
    if (/[0-9]/.test(c) || (c === '.' && /[0-9]/.test(src[i + 1]))) {
      // digit swizzle after a value: `.rgb0`-style constant components (e.g. `.000x`)
      const last = toks[toks.length - 1];
      if (c === '.' && last && (last.t === 'id' || (last.t === 'op' && last.v === ')'))) {
        let j = i + 1; while (j < n && /[0-9A-Za-z_]/.test(src[j])) j++;
        push('op', '.'); push('id', src.slice(i + 1, j)); i = j; continue;
      }
      let j = i; while (j < n && /[0-9.eE]/.test(src[j]) || (/[+\-]/.test(src[j]) && /[eE]/.test(src[j - 1]))) j++;
      const text = src.slice(i, j);
      let numv = Number(text);
      if (Number.isNaN(numv)) numv = parseFloat(text) || 0; // tolerate typos like "0.5.0"
      push('num', numv);
      if (j < n && (src[j] === 'f' || src[j] === 'F') && !/[A-Za-z0-9_]/.test(src[j + 1] || '')) j++; // C float suffix
      i = j; continue;
    }
    if (/[A-Za-z_]/.test(c)) {
      let j = i; while (j < n && /[A-Za-z0-9_]/.test(src[j])) j++;
      push('id', src.slice(i, j)); i = j; continue;
    }
    // multi-char operators
    const two = src.slice(i, i + 2);
    if (['==', '!=', '<=', '>=', '&&', '||', '+=', '-=', '*=', '/=', '%='].includes(two)) { push('op', two); i += 2; continue; }
    if ('+-*/=<>(){}[],.;!%?:'.includes(c)) { push('op', c); i++; continue; }
    // unknown char — skip defensively
    i++;
  }
  push('eof', null);
  return toks;
}

// ---------------- Parser ----------------
class Parser {
  constructor(toks) { this.toks = toks; this.i = 0; }
  peek(o = 0) { return this.toks[this.i + o]; }
  next() { return this.toks[this.i++]; }
  is(t, v) { const k = this.toks[this.i]; return k.t === t && (v === undefined || k.v === v); }
  eat(t, v) { if (!this.is(t, v)) throw new Error(`script parse: expected ${t + ' ' + (v ?? '')} got ${JSON.stringify(this.peek())}`); return this.next(); }

  // program := (global-decl | 'function' type? IDENT '(' ')' block)+
  // Tolerates stray tokens before the first function (";function", "4function", ...).
  parseProgram() {
    const funcs = new Map();
    const globals = [];
    let entry = null;
    for (;;) {
      // global declaration: TYPE IDENT '=' expr ';'
      if (this.is('id') && TYPE_KEYWORDS.has(this.peek().v) && this.peek(1).t === 'id' && !this.isFunctionAhead()) {
        globals.push(this.parseStatement());
        continue;
      }
      if (this.is('id', 'function')) {
        this.next();
        if (this.is('id') && TYPE_KEYWORDS.has(this.peek().v)) this.next(); // return type
        let name = 'Eval';
        if (this.is('id')) name = this.next().v;
        this.eat('op', '('); while (!this.is('op', ')')) this.next(); this.eat('op', ')');
        const block = this.parseBlock();
        funcs.set(name, block);
        if (entry == null || name === 'Eval') entry = name;
        continue;
      }
      if (this.is('op', '{') && !funcs.size) { funcs.set('Eval', this.parseBlock()); entry = 'Eval'; continue; }
      if (this.is('eof')) break;
      if (funcs.size) break;   // trailing junk after functions — done
      this.next();             // leading junk — skip
    }
    if (!funcs.size) throw new Error('script parse: no function body');
    return { funcs, entry, globals };
  }
  // lookahead guard: `function void Eval` also matches TYPE IDENT at 'void Eval'
  isFunctionAhead() { return this.is('id', 'function'); }
  parseBlock() {
    this.eat('op', '{');
    const stmts = [];
    while (!this.is('op', '}') && !this.is('eof')) stmts.push(this.parseStatement());
    this.eat('op', '}');
    return { k: 'block', stmts };
  }
  parseStatement() {
    if (this.is('op', '{')) return this.parseBlock();
    if (this.is('op', ';')) { this.next(); return { k: 'empty' }; }
    // declaration: TYPE IDENT ('=' expr)? terminator
    if (this.is('id') && TYPE_KEYWORDS.has(this.peek().v) && this.peek(1).t === 'id') {
      this.next(); // type
      const name = this.eat('id').v;
      let init = null;
      if (this.is('op', '=')) { this.next(); init = this.parseExpr(); }
      this.endStatement();
      return { k: 'decl', name, init };
    }
    // assignment or expression
    const lhs = this.parseExpr();
    if (this.is('op') && ['=', '+=', '-=', '*=', '/=', '%='].includes(this.peek().v)) {
      const op = this.next().v;
      const rhs = this.parseExpr();
      this.endStatement();
      return { k: 'assign', op, lhs, rhs };
    }
    this.endStatement();
    return { k: 'exprstmt', expr: lhs };
  }
  // statements end with ';' — tolerate a missing one before '}' (seen in the wild)
  endStatement() {
    if (this.is('op', ';')) { this.next(); return; }
    if (this.is('op', '}') || this.is('eof')) return;
    this.eat('op', ';');
  }

  // expression precedence climbing
  parseExpr() { return this.parseTernary(); }
  parseTernary() {
    let c = this.parseBin(0);
    if (this.is('op', '?')) { this.next(); const a = this.parseExpr(); this.eat('op', ':'); const b = this.parseExpr(); return { k: 'cond', c, a, b }; }
    return c;
  }
  parseBin(minPrec) {
    let left = this.parseUnary();
    for (;;) {
      const op = this.peek();
      const prec = BINPREC[op.v];
      if (op.t !== 'op' || prec === undefined || prec < minPrec) break;
      this.next();
      const right = this.parseBin(prec + 1);
      left = { k: 'bin', op: op.v, left, right };
    }
    return left;
  }
  parseUnary() {
    if (this.is('op', '-')) { this.next(); return { k: 'neg', e: this.parseUnary() }; }
    if (this.is('op', '!')) { this.next(); return { k: 'not', e: this.parseUnary() }; }
    if (this.is('op', '+')) { this.next(); return this.parseUnary(); }
    return this.parsePostfix();
  }
  parsePostfix() {
    let e = this.parsePrimary();
    for (;;) {
      if (this.is('op', '.')) {
        this.next();
        const member = this.eat('id').v;
        if (this.is('op', '(')) {
          const args = this.parseArgs();
          e = { k: 'method', obj: e, member, args };
        } else {
          e = { k: 'member', obj: e, member };
        }
      } else break;
    }
    return e;
  }
  parseArgs() {
    this.eat('op', '(');
    const args = [];
    while (!this.is('op', ')')) { args.push(this.parseExpr()); if (this.is('op', ',')) this.next(); }
    this.eat('op', ')');
    return args;
  }
  parsePrimary() {
    if (this.is('num')) return { k: 'num', v: this.next().v };
    if (this.is('op', '(')) { this.next(); const e = this.parseExpr(); this.eat('op', ')'); return e; }
    if (this.is('id')) {
      const name = this.next().v;
      if (this.is('op', '(')) { const args = this.parseArgs(); return { k: 'call', name, args }; }
      return { k: 'id', name };
    }
    throw new Error(`script parse: unexpected ${JSON.stringify(this.peek())}`);
  }
}
const BINPREC = { '||': 1, '&&': 2, '==': 3, '!=': 3, '<': 4, '>': 4, '<=': 4, '>=': 4, '+': 5, '-': 5, '*': 6, '/': 6, '%': 6 };

// ---------------- Swizzle helpers ----------------
// besides xyzw/rgba, PopcornFX allows the constants 0 and 1 (Color.rgb0, v.000x)
const SW = { x: 0, y: 1, z: 2, w: 3, r: 0, g: 1, b: 2, a: 3 };
function swizzle(v, s) {
  if (![...s].every((ch) => ch in SW || ch === '0' || ch === '1')) return null;
  return [...s].map((ch) => (ch === '0' ? 0 : ch === '1' ? 1 : (v[SW[ch]] ?? 0)));
}

// ---------------- Interpreter ----------------
// ctx provides: getField(name)->array|null, setField(name,array), hasField(name)->bool,
//   sampler(name)->{sample(...args)->array}|null, parentField(name)->array,
//   spawnerField(name)->array, rand(a,b), vrand(a,b), kill(), triggerEvent(name,args)
export function compileScript(src) {
  const prog = new Parser(tokenize(src)).parseProgram();
  const run = (ctx) => {
    const st = { funcs: prog.funcs, warned: run._warned };
    const locals = new Map();
    for (const g of prog.globals) execStmt(g, ctx, locals, st);
    execBlock(prog.funcs.get(prog.entry), ctx, locals, st);
  };
  run._warned = new Set();
  return { prog, run };
}

function execBlock(block, ctx, locals, st) {
  for (const s of block.stmts) execStmt(s, ctx, locals, st);
}
function execStmt(s, ctx, locals, st) {
  switch (s.k) {
    case 'block': return execBlock(s, ctx, new Map(locals), st);
    case 'empty': return;
    case 'decl': { const v = s.init ? evalExpr(s.init, ctx, locals, st) : [0]; locals.set(s.name, v.slice()); return; }
    case 'exprstmt': evalExpr(s.expr, ctx, locals, st); return;
    case 'assign': return execAssign(s, ctx, locals, st);
  }
}
function execAssign(s, ctx, locals, st) {
  const rhs = evalExpr(s.rhs, ctx, locals, st);
  // target: plain id, or a swizzle write (Color.rgb = ..., Position.x = ...) on an id
  let name, comps = null;
  if (s.lhs.k === 'id') name = s.lhs.name;
  else if (s.lhs.k === 'member' && s.lhs.obj.k === 'id') {
    name = s.lhs.obj.name;
    comps = [...s.lhs.member].map((ch) => SW[ch]);
    if (comps.some((c) => c === undefined)) return; // not a swizzle target — ignore
  } else return; // unsupported target (e.g. parent.X) — ignore rather than kill the sim

  const cur = locals.has(name) ? locals.get(name) : (ctx.getField(name) ?? [0]);
  let next;
  if (comps) {
    next = cur.slice();
    while (next.length <= Math.max(...comps)) next.push(0);
    const curSel = comps.map((c) => next[c] ?? 0);
    const applied = applyCompound(s.op, curSel, rhs);
    comps.forEach((c, k) => { next[c] = applied[k] ?? applied[0] ?? 0; });
  } else {
    next = applyCompound(s.op, cur, rhs);
  }
  if (locals.has(name)) locals.set(name, next.slice());
  else ctx.setField(name, next);
}
function applyCompound(op, cur, rhs) {
  switch (op) {
    case '=': return rhs.slice();
    case '+=': return vadd(cur, rhs);
    case '-=': return vsub(cur, rhs);
    case '*=': return vmul(cur, rhs);
    case '/=': return vdiv(cur, rhs);
    case '%=': return ew(cur, rhs, (x, y) => x % y);
  }
}

function evalExpr(e, ctx, locals, st) {
  switch (e.k) {
    case 'num': return [e.v];
    case 'neg': return vneg(evalExpr(e.e, ctx, locals, st));
    case 'not': return [evalExpr(e.e, ctx, locals, st)[0] ? 0 : 1];
    case 'id': return evalId(e.name, ctx, locals);
    case 'bin': return evalBin(e, ctx, locals, st);
    case 'cond': return evalExpr(e.c, ctx, locals, st)[0] ? evalExpr(e.a, ctx, locals, st) : evalExpr(e.b, ctx, locals, st);
    case 'member': return evalMember(e, ctx, locals, st);
    case 'method': return evalMethod(e, ctx, locals, st);
    case 'call': return evalCall(e, ctx, locals, st);
  }
  throw new Error('script: bad node ' + e.k);
}
function evalId(name, ctx, locals) {
  if (locals.has(name)) return locals.get(name);
  if (name === 'pi' || name === 'PI') return [PI];
  const f = ctx.getField(name);
  if (f) return f;
  const a = ctx.attribute ? ctx.attribute(name) : null;
  if (a) return a;
  return [0];
}
function evalBin(e, ctx, locals, st) {
  const a = evalExpr(e.left, ctx, locals, st), b = evalExpr(e.right, ctx, locals, st);
  switch (e.op) {
    case '+': return vadd(a, b); case '-': return vsub(a, b);
    case '*': return vmul(a, b); case '/': return vdiv(a, b);
    case '%': return ew(a, b, (x, y) => x % y);
    case '<': return [a[0] < b[0] ? 1 : 0]; case '>': return [a[0] > b[0] ? 1 : 0];
    case '<=': return [a[0] <= b[0] ? 1 : 0]; case '>=': return [a[0] >= b[0] ? 1 : 0];
    case '==': return [a[0] === b[0] ? 1 : 0]; case '!=': return [a[0] !== b[0] ? 1 : 0];
    case '&&': return [a[0] && b[0] ? 1 : 0]; case '||': return [a[0] || b[0] ? 1 : 0];
  }
}
function evalMember(e, ctx, locals, st) {
  // parent.Field / spawner.Field  OR  vectorValue.swizzle
  if (e.obj.k === 'id' && e.obj.name === 'parent') return ctx.parentField ? (ctx.parentField(e.member) ?? [0]) : [0];
  if (e.obj.k === 'id' && e.obj.name === 'spawner') return ctx.spawnerField ? (ctx.spawnerField(e.member) ?? [0]) : [0];
  const base = evalExpr(e.obj, ctx, locals, st);
  const sw = swizzle(base, e.member);
  if (sw) return sw;
  return [0];
}
function evalMethod(e, ctx, locals, st) {
  // sampler method: Curve.sample(t [,sel]) ; Shape.samplePosition() ; scene.axisUp() ...
  const objName = e.obj.k === 'id' ? e.obj.name : null;
  const args = e.args.map((a) => evalExpr(a, ctx, locals, st));
  if (objName === 'scene' || objName === 'fast') {
    const fn = SCENE[e.member] || INTRINSICS[e.member];
    if (fn) return fn(...args);
    return [0];
  }
  if (objName) {
    // EventName.trigger(cond) — fire a declared script event
    if (e.member === 'trigger' && ctx.triggerEvent) { ctx.triggerEvent(objName, args); return [0]; }
    const sampler = ctx.sampler ? ctx.sampler(objName) : null;
    if (sampler && typeof sampler[e.member] === 'function') return sampler[e.member](...args);
    if (sampler && e.member === 'sample') return sampler.sample(...args);
  }
  // fallback: maybe it's swizzle-like or unknown -> 0
  return [0];
}
function evalCall(e, ctx, locals, st) {
  // helper functions defined in the same script (Run, PostEval, ...)
  if (st.funcs.has(e.name)) { execBlock(st.funcs.get(e.name), ctx, new Map(locals), st); return [0]; }
  const fn = INTRINSICS[e.name];
  const args = e.args.map((a) => evalExpr(a, ctx, locals, st));
  if (e.name === 'rand') return [ctx.rand(args[0] ? args[0][0] : 0, args[1] ? args[1][0] : 1)];
  if (e.name === 'vrand') return ctx.vrand(args[0] ? args[0][0] : -1, args[1] ? args[1][0] : 1);
  if (e.name === 'randsel') { const idx = Math.floor(ctx.rand(0, args.length)); return args[Math.min(idx, args.length - 1)]; }
  if (e.name === 'kill') { ctx.kill(); return [0]; }
  if (e.name === 'trigger') { if (ctx.trigger) ctx.trigger(args); return [0]; }
  if (fn) return fn(...args);
  // unknown function: warn once, keep the simulation alive
  if (!st.warned.has(e.name)) { st.warned.add(e.name); console.warn('pkfx script: unknown function', e.name); }
  return [0];
}

// ---------------- Intrinsics ----------------
const clamp1 = (x, a, b) => Math.min(Math.max(x, a), b);
function sat(v) { return v.map((x) => clamp1(x, 0, 1)); }
function vlen(v) { let s = 0; for (const x of v) s += x * x; return Math.sqrt(s); }
function vnorm(v) { const l = vlen(v) || 1; return v.map((x) => x / l); }
function vdot(a, b) { const [x, y, n] = broadcast(a, b); let s = 0; for (let i = 0; i < n; i++) s += x[i] * y[i]; return [s]; }
function vcross(a, b) {
  const a0 = a[0] ?? 0, a1 = a[1] ?? 0, a2 = a[2] ?? 0;
  const b0 = b[0] ?? 0, b1 = b[1] ?? 0, b2 = b[2] ?? 0;
  return [a1 * b2 - a2 * b1, a2 * b0 - a0 * b2, a0 * b1 - a1 * b0];
}
function mklerp(a, b, t) { const [x, y, n] = broadcast(a, b); const tt = t.length === 1 ? new Array(n).fill(t[0]) : t; const o = new Array(n); for (let i = 0; i < n; i++) o[i] = x[i] + (y[i] - x[i]) * tt[i]; return o; }
function smoothstep1(a, b, x) { const t = clamp1((x - a) / ((b - a) || 1e-9), 0, 1); return t * t * (3 - 2 * t); }

function rgb2hsv(c) {
  const r = c[0], g = c[1], b = c[2];
  const mx = Math.max(r, g, b), mn = Math.min(r, g, b), d = mx - mn;
  let h = 0; if (d !== 0) { if (mx === r) h = ((g - b) / d) % 6; else if (mx === g) h = (b - r) / d + 2; else h = (r - g) / d + 4; }
  h /= 6; if (h < 0) h += 1;
  const s = mx === 0 ? 0 : d / mx;
  return [h, s, mx];
}
function hsv2rgb(c) {
  const h0 = Number.isFinite(c[0]) ? c[0] : 0;
  const h = ((h0 % 1) + 1) % 1, s = c[1] || 0, v = c[2] || 0;
  const i = Math.floor(h * 6), f = h * 6 - i;
  const p = v * (1 - s), q = v * (1 - f * s), t = v * (1 - (1 - f) * s);
  const m = [[v, t, p], [q, v, p], [p, v, t], [p, q, v], [t, p, v], [v, p, q]][i % 6];
  return [m[0], m[1], m[2]];
}

const INTRINSICS = {
  float: (a) => [a ? a[0] : 0],
  float2: (...a) => mkvec(a, 2),
  float3: (...a) => mkvec(a, 3),
  float4: (...a) => mkvec(a, 4),
  int: (a) => [Math.trunc(a ? a[0] : 0)],
  int2: (...a) => mkvec(a, 2).map(Math.trunc),
  int3: (...a) => mkvec(a, 3).map(Math.trunc),
  int4: (...a) => mkvec(a, 4).map(Math.trunc),
  sin: (a) => a.map(Math.sin), cos: (a) => a.map(Math.cos), tan: (a) => a.map(Math.tan),
  asin: (a) => a.map(Math.asin), acos: (a) => a.map(Math.acos), atan: (a) => a.map(Math.atan),
  atan2: (a, b) => ew(a, b, Math.atan2),
  abs: (a) => a.map(Math.abs), floor: (a) => a.map(Math.floor), ceil: (a) => a.map(Math.ceil),
  sign: (a) => a.map(Math.sign), sqrt: (a) => a.map(Math.sqrt), frac: (a) => a.map((x) => x - Math.floor(x)),
  exp: (a) => a.map(Math.exp), log: (a) => a.map(Math.log),
  pow: (a, b) => ew(a, b, (x, y) => Math.pow(x, y)),
  min: (a, b) => ew(a, b, Math.min), max: (a, b) => ew(a, b, Math.max),
  clamp: (a, lo, hi) => { const [x1, l1, n] = broadcast(a, lo); const hh = hi.length === 1 ? new Array(n).fill(hi[0]) : hi; return x1.map((x, i) => clamp1(x, l1[i], hh[i])); },
  saturate: (a) => sat(a),
  lerp: (a, b, t) => mklerp(a, b, t),
  smoothlerp: (a, b, t) => mklerp(a, b, [smoothstep1(0, 1, t[0])]),
  smoothstep: (a, b, x) => [smoothstep1(a[0], b[0], x[0])],
  normalize: (a) => vnorm(a),
  length: (a) => [vlen(a)],
  dot: (a, b) => vdot(a, b),
  cross: (a, b) => vcross(a, b),
  distance: (a, b) => [vlen(vsub(a, b))],
  deg2rad: (a) => a.map((x) => x * PI / 180), rad2deg: (a) => a.map((x) => x * 180 / PI),
  rgb2hsv: (a) => rgb2hsv(a), hsv2rgb: (a) => hsv2rgb(a),
  linear2srgb: (a) => a.map((x) => x <= 0.0031308 ? x * 12.92 : 1.055 * Math.pow(x, 1 / 2.4) - 0.055),
  srgb2linear: (a) => a.map((x) => x <= 0.04045 ? x / 12.92 : Math.pow((x + 0.055) / 1.055, 2.4)),
  bias: (x, e) => x.map((v) => Math.pow(clamp1(v, 0, 1), Math.pow(2, -e[0]))), // bias is defined on [0,1]
  select: (c, a, b) => (c[0] ? a : b),
  iif: (c, a, b) => (c[0] ? a : b),
  rotate: (v, axis, angle) => rotateAxis(v, vnorm(axis), angle[0]),
  wavesq: (a) => a.map((x) => ((x - Math.floor(x)) < 0.5 ? 1 : -1)), // unit square wave
};
function mkvec(args, n) {
  // float3(a) -> broadcast scalar; float3(x,y,z) -> components; float4(vec3, w) -> concat
  const flat = [];
  for (const a of args) for (const c of a) flat.push(c);
  if (flat.length === 1) return new Array(n).fill(flat[0]);
  const o = new Array(n).fill(0);
  for (let i = 0; i < n; i++) o[i] = flat[i] ?? 0;
  return o;
}
function rotateAxis(v, k, ang) {
  // Rodrigues' rotation of vector v about unit axis k by angle ang
  const c = Math.cos(ang), s = Math.sin(ang);
  const kv = vcross(k, v); const kd = k[0] * v[0] + k[1] * v[1] + k[2] * v[2];
  return [
    v[0] * c + kv[0] * s + k[0] * kd * (1 - c),
    v[1] * c + kv[1] * s + k[1] * kd * (1 - c),
    v[2] * c + kv[2] * s + k[2] * kd * (1 - c),
  ];
}
const SCENE = {
  axisUp: () => [0, 1, 0], axisForward: () => [0, 0, 1], axisSide: () => [1, 0, 0],
  axisDown: () => [0, -1, 0], axisBackward: () => [0, 0, -1],
  linear2srgb: INTRINSICS.linear2srgb,
};

export { vadd, vsub, vmul, vdiv, vlen, vnorm, rgb2hsv, hsv2rgb };

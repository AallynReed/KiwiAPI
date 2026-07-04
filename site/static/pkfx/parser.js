// Parser for PopcornFX v1.x (.pkfx) text serialization.
//
// Grammar (informal):
//   file    := header* object*
//   header  := IDENT '=' <raw-until-';'> ';'        e.g. Version = 1.13.4.53415;
//   object  := IDENT IDENT '{' member* '}'          e.g. CParticleEffect  $LOCAL$/Resource { ... }
//   member  := IDENT '=' value ';'
//   value   := number | bool | string | ctor | list | symbol
//   ctor    := IDENT '(' value (',' value)* ')'      e.g. float3(1, 0, 0)
//   list    := '{' (value ',')* '}'
//   string  := '"' ( \\" | \\\\ | any )* '"'         (may span multiple lines — used for scripts)
//
// The parser is char-driven so multi-line script strings and nested braces are handled robustly.
// Values are decoded into a lossless tagged JS representation; see decodeValue cases below.

const ID_CHARS = /[A-Za-z0-9_$/.]/;
const WORD_START = /[A-Za-z_$/]/;

export class PkfxParseError extends Error {
  constructor(message, pos, src) {
    const { line, col } = lineCol(src, pos);
    super(`${message} (line ${line}, col ${col})`);
    this.name = 'PkfxParseError';
    this.pos = pos;
    this.line = line;
    this.col = col;
  }
}

function lineCol(src, pos) {
  let line = 1, col = 1;
  for (let i = 0; i < pos && i < src.length; i++) {
    if (src[i] === '\n') { line++; col = 1; } else { col++; }
  }
  return { line, col };
}

/**
 * Parse a .pkfx document.
 * @param {string} src raw file text
 * @returns {{version:string|null, generator:string|null, order:string[], objects:Object<string,{className:string,id:string,props:Object}>}}
 */
export function parsePkfx(src) {
  const p = new Cursor(src);
  const doc = { version: null, generator: null, order: [], objects: Object.create(null) };

  p.skipWs();
  while (!p.eof()) {
    // A top-level statement starts with an identifier. It is either:
    //   IDENT '=' ...        (a header assignment), or
    //   IDENT IDENT '{' ...  (an object definition).
    const word = p.readWord();
    if (word == null) {
      throw new PkfxParseError(`Expected identifier at top level, got ${JSON.stringify(p.peek())}`, p.i, src);
    }
    p.skipWs();
    if (p.peek() === '=') {
      p.next(); // '='
      const raw = p.readRawUntil(';').trim();
      p.expect(';');
      if (word === 'Version') doc.version = raw;
      else if (word === 'Generator') doc.generator = raw;
      // unknown headers are ignored but tolerated
    } else {
      // object definition: word is the class name, next word is the id
      const id = p.readWord();
      if (id == null) throw new PkfxParseError(`Expected object id after class ${word}`, p.i, src);
      p.skipWs();
      p.expect('{');
      const props = p.readBlock();
      const obj = { className: word, id, props };
      if (id in doc.objects) {
        // duplicate id — keep first, but this should not happen in valid files
        doc.objects[id]._duplicate = true;
      } else {
        doc.order.push(id);
      }
      doc.objects[id] = obj;
    }
    p.skipWs();
  }

  return doc;
}

class Cursor {
  constructor(src) { this.s = src; this.i = 0; this.n = src.length; }
  eof() { return this.i >= this.n; }
  peek(o = 0) { return this.s[this.i + o]; }
  next() { return this.s[this.i++]; }

  err(msg) { return new PkfxParseError(msg, this.i, this.s); }
  expect(ch) {
    this.skipWs();
    if (this.s[this.i] !== ch) throw this.err(`Expected '${ch}' but found ${JSON.stringify(this.s[this.i] ?? '<eof>')}`);
    this.i++;
  }

  skipWs() {
    while (this.i < this.n) {
      const c = this.s[this.i];
      // skip spaces and any control character (some files contain stray 0x08 etc.)
      if (c <= ' ') { this.i++; continue; }
      // line comments are not part of the serialization grammar (only inside script strings),
      // but tolerate them defensively just in case.
      if (c === '/' && this.s[this.i + 1] === '/') {
        while (this.i < this.n && this.s[this.i] !== '\n') this.i++;
        continue;
      }
      break;
    }
  }

  readWord() {
    this.skipWs();
    if (this.i >= this.n || !WORD_START.test(this.s[this.i])) return null;
    const start = this.i;
    while (this.i < this.n && ID_CHARS.test(this.s[this.i])) this.i++;
    return this.s.slice(start, this.i);
  }

  readRawUntil(stop) {
    const start = this.i;
    while (this.i < this.n && this.s[this.i] !== stop) this.i++;
    return this.s.slice(start, this.i);
  }

  // Read the body of an object or nested block: '{' already consumed. Returns props object.
  readBlock() {
    const props = Object.create(null);
    this.skipWs();
    while (this.i < this.n && this.s[this.i] !== '}') {
      const key = this.readWord();
      if (key == null) throw this.err('Expected property name in block');
      this.skipWs();
      this.expect('=');
      const val = this.readValue();
      this.skipWs();
      // members are terminated by ';' (lists/blocks may already have consumed their close)
      if (this.s[this.i] === ';') this.i++;
      props[key] = val;
      this.skipWs();
    }
    this.expect('}');
    return props;
  }

  readValue() {
    this.skipWs();
    const c = this.s[this.i];
    if (c === '"') return this.readString();
    if (c === '{') return this.readList();
    if (c === '-' || c === '+' || c === '.' || (c >= '0' && c <= '9')) return this.readNumber();
    // identifier: could be a ctor (float3(...)) or a bare symbol/bool (AlphaBlend, true)
    const word = this.readWord();
    if (word == null) throw this.err(`Unexpected value start ${JSON.stringify(c)}`);
    this.skipWs();
    if (this.s[this.i] === '(') {
      this.i++; // '('
      const args = [];
      this.skipWs();
      while (this.i < this.n && this.s[this.i] !== ')') {
        args.push(this.readValue());
        this.skipWs();
        if (this.s[this.i] === ',') { this.i++; this.skipWs(); }
      }
      this.expect(')');
      return { ctor: word, args };
    }
    if (word === 'true') return true;
    if (word === 'false') return false;
    return { sym: word };
  }

  readString() {
    // opening quote
    this.i++;
    let out = '';
    while (this.i < this.n) {
      const c = this.s[this.i++];
      if (c === '\\') {
        const e = this.s[this.i++];
        if (e === 'n') out += '\n';
        else if (e === 't') out += '\t';
        else if (e === 'r') out += '\r';
        else out += e; // \" \\ and anything else -> literal
        continue;
      }
      if (c === '"') return out;
      out += c;
    }
    throw this.err('Unterminated string');
  }

  readList() {
    this.i++; // '{'
    const arr = [];
    this.skipWs();
    while (this.i < this.n && this.s[this.i] !== '}') {
      arr.push(this.readValue());
      this.skipWs();
      if (this.s[this.i] === ',') { this.i++; this.skipWs(); }
    }
    this.expect('}');
    return arr;
  }

  readNumber() {
    const start = this.i;
    // A numeric token runs until a value terminator. This tolerates the data's
    // quirks: MSVC infinities (1.#INF000e+000), indeterminates (-1.#IND000e+000),
    // and C float suffixes (1000.0f).
    while (this.i < this.n && !/[;,)}\s]/.test(this.s[this.i])) this.i++;
    const text = this.s.slice(start, this.i);
    return interpretNumber(text, start, this.s);
  }
}

// Interpret a numeric token, handling MSVC special forms and the C 'f' suffix.
function interpretNumber(text, start, src) {
  const lower = text.toLowerCase();
  const neg = lower[0] === '-';
  if (lower.includes('#inf')) return neg ? -Infinity : Infinity;
  if (lower.includes('#ind') || lower.includes('#qnan') || lower.includes('#snan') || lower.includes('nan')) return NaN;
  // strip a trailing float suffix (1000.0f / 1.0F)
  const cleaned = text.replace(/[fF]$/, '');
  const num = Number(cleaned);
  if (Number.isNaN(num)) throw new PkfxParseError(`Invalid number ${JSON.stringify(text)}`, start, src);
  return num;
}

// ---- Convenience helpers for working with the decoded graph ----

const LOCAL = '$LOCAL$/';

/** True if a decoded value is a same-file object reference string. */
export function isLocalRef(v) {
  return typeof v === 'string' && v.startsWith(LOCAL);
}

/** Resolve a property value that is a local ref into its object (or null). */
export function deref(doc, v) {
  if (isLocalRef(v)) return doc.objects[v] ?? null;
  return null;
}

/** Flatten a ctor value (floatN/intN) to a plain number array; pass through arrays/numbers. */
export function toNums(v) {
  if (typeof v === 'number') return [v];
  if (v && v.ctor) return v.args.map((a) => (typeof a === 'number' ? a : Number(a)));
  if (Array.isArray(v)) return v.map((a) => (typeof a === 'number' ? a : Number(a)));
  return null;
}

/** Get a bare symbol/enum name from a decoded value, or null. */
export function toSym(v) {
  if (v && typeof v === 'object' && 'sym' in v) return v.sym;
  return null;
}

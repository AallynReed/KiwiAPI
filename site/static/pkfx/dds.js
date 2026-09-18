// Minimal DDS decoder -> RGBA8. Handles the formats present in the corpus:
// BC1/DXT1, BC2/DXT3, BC3/DXT5 (the vast majority), BC7 and uncompressed 24/32-bit.
// Returns { width, height, rgba: Uint8ClampedArray }.

const FOURCC = (s) => s.charCodeAt(0) | (s.charCodeAt(1) << 8) | (s.charCodeAt(2) << 16) | (s.charCodeAt(3) << 24);
const MAGIC = FOURCC('DDS ');

export function decodeDDS(buffer) {
  const view = new DataView(buffer);
  if (view.getUint32(0, true) !== MAGIC) throw new Error('not a DDS file');
  const height = view.getUint32(12, true);
  const width = view.getUint32(16, true);
  const pfFlags = view.getUint32(80, true);
  const fourCC = view.getUint32(84, true);
  const rgbBits = view.getUint32(88, true);
  const rMask = view.getUint32(92, true), gMask = view.getUint32(96, true), bMask = view.getUint32(100, true), aMask = view.getUint32(104, true);

  let dataOffset = 128;
  const DDPF_FOURCC = 0x4;
  const rgba = new Uint8ClampedArray(width * height * 4);

  if (pfFlags & DDPF_FOURCC) {
    if (fourCC === FOURCC('DXT1')) decodeBC(view, dataOffset, width, height, rgba, 1);
    else if (fourCC === FOURCC('DXT3')) decodeBC(view, dataOffset, width, height, rgba, 2);
    else if (fourCC === FOURCC('DXT5')) decodeBC(view, dataOffset, width, height, rgba, 3);
    else if (fourCC === FOURCC('DX10')) {
      const dxgi = view.getUint32(128, true); dataOffset = 148;
      // 71/72=BC1, 74/75=BC2, 77/78=BC3
      if (dxgi === 71 || dxgi === 72) decodeBC(view, dataOffset, width, height, rgba, 1);
      else if (dxgi === 74 || dxgi === 75) decodeBC(view, dataOffset, width, height, rgba, 2);
      else if (dxgi === 77 || dxgi === 78) decodeBC(view, dataOffset, width, height, rgba, 3);
      else if (dxgi === 98 || dxgi === 99) decodeBC7(view, dataOffset, width, height, rgba);
      else throw new Error('unsupported DX10 dxgiFormat ' + dxgi);
    } else throw new Error('unsupported FourCC ' + fourCC.toString(16));
  } else {
    decodeUncompressed(view, dataOffset, width, height, rgba, rgbBits, rMask, gMask, bMask, aMask);
  }
  return { width, height, rgba };
}

function color565(c, out, o) {
  out[o] = ((c >> 11) & 0x1f) * 255 / 31;
  out[o + 1] = ((c >> 5) & 0x3f) * 255 / 63;
  out[o + 2] = (c & 0x1f) * 255 / 31;
}

// bcType: 1=BC1, 2=BC2, 3=BC3
function decodeBC(view, offset, width, height, rgba, bcType) {
  const blockBytes = bcType === 1 ? 8 : 16;
  const bw = (width + 3) >> 2, bh = (height + 3) >> 2;
  const c = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]; // 4 colors x rgb
  let p = offset;
  for (let by = 0; by < bh; by++) {
    for (let bx = 0; bx < bw; bx++) {
      let cOff = p;
      const alpha = new Uint8Array(16);
      if (bcType === 2) {
        for (let i = 0; i < 8; i++) { const b = view.getUint8(p + i); alpha[i * 2] = (b & 0xf) * 17; alpha[i * 2 + 1] = (b >> 4) * 17; }
        cOff = p + 8;
      } else if (bcType === 3) {
        const a0 = view.getUint8(p), a1 = view.getUint8(p + 1);
        const aIdx = []; let bits = 0n;
        for (let i = 0; i < 6; i++) bits |= BigInt(view.getUint8(p + 2 + i)) << BigInt(8 * i);
        for (let i = 0; i < 16; i++) { const code = Number((bits >> BigInt(3 * i)) & 7n); alpha[i] = bc3Alpha(a0, a1, code); }
        cOff = p + 8;
      }
      const c0 = view.getUint16(cOff, true), c1 = view.getUint16(cOff + 2, true);
      color565(c0, c, 0); color565(c1, c, 3);
      if (bcType === 1 && c0 <= c1) {
        for (let k = 0; k < 3; k++) { c[6 + k] = (c[k] + c[3 + k]) / 2; c[9 + k] = 0; }
      } else {
        for (let k = 0; k < 3; k++) { c[6 + k] = (2 * c[k] + c[3 + k]) / 3; c[9 + k] = (c[k] + 2 * c[3 + k]) / 3; }
      }
      const idx = view.getUint32(cOff + 4, true);
      for (let py = 0; py < 4; py++) {
        for (let px = 0; px < 4; px++) {
          const x = bx * 4 + px, y = by * 4 + py; if (x >= width || y >= height) continue;
          const ci = (idx >> (2 * (py * 4 + px))) & 3;
          const o = (y * width + x) * 4;
          rgba[o] = c[ci * 3]; rgba[o + 1] = c[ci * 3 + 1]; rgba[o + 2] = c[ci * 3 + 2];
          let a = 255;
          if (bcType === 1) { a = (c0 <= c1 && ci === 3) ? 0 : 255; }
          else a = alpha[py * 4 + px];
          rgba[o + 3] = a;
        }
      }
      p += blockBytes;
    }
  }
}
function bc3Alpha(a0, a1, code) {
  if (code === 0) return a0; if (code === 1) return a1;
  if (a0 > a1) return ((8 - code) * a0 + (code - 1) * a1) / 7;
  if (code === 6) return 0; if (code === 7) return 255;
  return ((6 - code) * a0 + (code - 1) * a1) / 5;
}

function decodeUncompressed(view, offset, width, height, rgba, bits, rMask, gMask, bMask, aMask) {
  const bytes = bits / 8;
  const shift = (mask) => { if (!mask) return 0; let s = 0; while (!((mask >> s) & 1)) s++; return s; };
  const sr = shift(rMask), sg = shift(gMask), sb = shift(bMask), sa = shift(aMask);
  const mr = rMask >>> sr, mg = gMask >>> sg, mb = bMask >>> sb, ma = aMask >>> sa;
  for (let y = 0; y < height; y++) for (let x = 0; x < width; x++) {
    const p = offset + (y * width + x) * bytes;
    let px = 0; for (let b = 0; b < bytes; b++) px |= view.getUint8(p + b) << (8 * b);
    const o = (y * width + x) * 4;
    rgba[o] = mr ? ((px & rMask) >>> sr) * 255 / mr : 0;
    rgba[o + 1] = mg ? ((px & gMask) >>> sg) * 255 / mg : 0;
    rgba[o + 2] = mb ? ((px & bMask) >>> sb) * 255 / mb : 0;
    rgba[o + 3] = ma ? ((px & aMask) >>> sa) * 255 / ma : 255;
  }
}

// ---- BC7 (BPTC), per the D3D11 format spec. Tables: 2-subset partitions as 16-bit
// masks, 3-subset partitions at 2 bits per pixel, anchor indices, interpolation weights.
const BC7_P2 = [0xcccc,0x8888,0xeeee,0xecc8,0xc880,0xfeec,0xfec8,0xec80,0xc800,0xffec,0xfe80,0xe800,0xffe8,0xff00,0xfff0,0xf000,0xf710,0x008e,0x7100,0x08ce,0x008c,0x7310,0x3100,0x8cce,0x088c,0x3110,0x6666,0x366c,0x17e8,0x0ff0,0x718e,0x399c,0xaaaa,0xf0f0,0x5a5a,0x33cc,0x3c3c,0x55aa,0x9696,0xa55a,0x73ce,0x13c8,0x324c,0x3bdc,0x6996,0xc33c,0x9966,0x0660,0x0272,0x04e4,0x4e40,0x2720,0xc936,0x936c,0x39c6,0x639c,0x9336,0x9cc6,0x817e,0xe718,0xccf0,0x0fcc,0x7744,0xee22];
const BC7_P3 = [0xaa685050,0x6a5a5040,0x5a5a4200,0x5450a0a8,0xa5a50000,0xa0a05050,0x5555a0a0,0x5a5a5050,0xaa550000,0xaa555500,0xaaaa5500,0x90909090,0x94949494,0xa4a4a4a4,0xa9a59450,0x2a0a4250,0xa5945040,0x0a425054,0xa5a5a500,0x55a0a0a0,0xa8a85454,0x6a6a4040,0xa4a45000,0x1a1a0500,0x0050a4a4,0xaaa59090,0x14696914,0x69691400,0xa08585a0,0xaa821414,0x50a4a450,0x6a5a0200,0xa9a58000,0x5090a0a8,0xa8a09050,0x24242424,0x00aa5500,0x24924924,0x24499224,0x50a50a50,0x500aa550,0xaaaa4444,0x66660000,0xa5a0a5a0,0x50a050a0,0x69286928,0x44aaaa44,0x66666600,0xaa444444,0x54a854a8,0x95809580,0x96969600,0xa85454a8,0x80959580,0xaa141414,0x96960000,0xaaaa1414,0xa05050a0,0xa0a5a5a0,0x96000000,0x40804080,0xa9a8a9a8,0xaaaaaa44,0x2a4a5254];
const BC7_A2 = [15,15,15,15,15,15,15,15,15,15,15,15,15,15,15,15,15,2,8,2,2,8,8,15,2,8,2,2,8,8,2,2,15,15,6,8,2,8,15,15,2,8,2,2,2,15,15,6,6,2,6,8,15,15,2,2,15,15,15,15,15,2,2,15];
const BC7_A3A = [3,3,15,15,8,3,15,15,8,8,6,6,6,5,3,3,3,3,8,15,3,3,6,10,5,8,8,6,8,5,15,15,8,15,3,5,6,10,8,15,15,3,15,5,15,15,15,15,3,15,5,5,5,8,5,10,5,10,8,13,15,12,3,3];
const BC7_A3B = [15,8,8,3,15,15,3,8,15,15,15,15,15,15,15,8,15,8,15,3,15,8,15,8,3,15,6,10,15,15,10,8,15,3,15,10,10,8,9,10,6,15,8,15,3,6,6,8,15,3,15,15,15,15,15,15,15,15,15,15,3,15,15,8];
const BC7_W = { 2: [0,21,43,64], 3: [0,9,18,27,37,46,55,64], 4: [0,4,9,13,17,21,26,30,34,38,43,47,51,55,60,64] };
// subsets, partition bits, rotation bits, index-selection bit, color bits, alpha bits,
// per-endpoint p-bit, shared p-bit, index bits, secondary index bits
const BC7_MODES = [
  [3, 4, 0, 0, 4, 0, 1, 0, 3, 0], [2, 6, 0, 0, 6, 0, 0, 1, 3, 0],
  [3, 6, 0, 0, 5, 0, 0, 0, 2, 0], [2, 6, 0, 0, 7, 0, 1, 0, 2, 0],
  [1, 0, 2, 1, 5, 6, 0, 0, 2, 3], [1, 0, 2, 0, 7, 8, 0, 0, 2, 2],
  [1, 0, 0, 0, 7, 7, 1, 0, 4, 0], [2, 6, 0, 0, 5, 5, 1, 0, 2, 0],
];

function decodeBC7(view, offset, width, height, rgba) {
  const bw = (width + 3) >> 2, bh = (height + 3) >> 2;
  const words = new Uint32Array(4), ep = new Uint8Array(3 * 2 * 4), px = new Uint8Array(64);
  let pos = 0;
  const bits = (n) => {             // LSB-first over the 128-bit block
    let v = 0;
    for (let k = 0; k < n; k++, pos++) v |= ((words[pos >> 5] >>> (pos & 31)) & 1) << k;
    return v;
  };
  let p = offset;
  for (let by = 0; by < bh; by++) for (let bx = 0; bx < bw; bx++, p += 16) {
    for (let k = 0; k < 4; k++) words[k] = view.getUint32(p + 4 * k, true);
    pos = 0;
    let mode = 0;
    while (mode < 8 && !bits(1)) mode++;
    px.fill(0);
    if (mode < 8) decodeBC7Block(mode, bits, ep, px);
    for (let y = 0; y < 4; y++) for (let x = 0; x < 4; x++) {
      const X = bx * 4 + x, Y = by * 4 + y;
      if (X >= width || Y >= height) continue;
      const o = (Y * width + X) * 4, i = (y * 4 + x) * 4;
      rgba[o] = px[i]; rgba[o + 1] = px[i + 1]; rgba[o + 2] = px[i + 2]; rgba[o + 3] = px[i + 3];
    }
  }
}

function decodeBC7Block(mode, bits, ep, px) {
  const [ns, pb, rb, isb, cb, ab, epb, spb, ib, ib2] = BC7_MODES[mode];
  const part = bits(pb), rot = bits(rb), sel = bits(isb);
  // endpoints: ep[(subset*2 + end)*4 + channel], channels r g b then a
  for (let c = 0; c < 3; c++) for (let s = 0; s < ns; s++) for (let e = 0; e < 2; e++) ep[(s * 2 + e) * 4 + c] = bits(cb);
  if (ab) for (let s = 0; s < ns; s++) for (let e = 0; e < 2; e++) ep[(s * 2 + e) * 4 + 3] = bits(ab);
  const pbit = new Array(ns * 2).fill(0);
  if (epb) for (let k = 0; k < ns * 2; k++) pbit[k] = bits(1);
  if (spb) for (let s = 0; s < ns; s++) { const b = bits(1); pbit[s * 2] = b; pbit[s * 2 + 1] = b; }
  const expand = (v, n) => { v <<= 8 - n; return v | (v >> n); };
  for (let k = 0; k < ns * 2; k++) for (let c = 0; c < 4; c++) {
    const o = k * 4 + c;
    if (c === 3 && !ab) { ep[o] = 255; continue; }
    let v = ep[o], n = c === 3 ? ab : cb;
    if (epb || spb) { v = (v << 1) | pbit[k]; n++; }
    ep[o] = expand(v, n);
  }
  const subsetOf = (i) => ns === 2 ? (BC7_P2[part] >> i) & 1 : ns === 3 ? (BC7_P3[part] >>> (2 * i)) & 3 : 0;
  const isAnchor = (i) => i === 0 || (ns === 2 && i === BC7_A2[part]) || (ns === 3 && (i === BC7_A3A[part] || i === BC7_A3B[part]));
  const idx = new Array(16), idx2 = new Array(16);
  for (let i = 0; i < 16; i++) idx[i] = bits(isAnchor(i) ? ib - 1 : ib);
  if (ib2) for (let i = 0; i < 16; i++) idx2[i] = bits(i === 0 ? ib2 - 1 : ib2);
  const wc = BC7_W[ib2 && sel ? ib2 : ib], wa = BC7_W[ib2 && !sel ? ib2 : ib];
  for (let i = 0; i < 16; i++) {
    const s = subsetOf(i), e0 = s * 8, e1 = s * 8 + 4;
    const ci = ib2 ? (sel ? idx2[i] : idx[i]) : idx[i];
    const ai = ib2 ? (sel ? idx[i] : idx2[i]) : idx[i];
    const w = wc[ci], wA = wa[ai];
    for (let c = 0; c < 3; c++) px[i * 4 + c] = ((64 - w) * ep[e0 + c] + w * ep[e1 + c] + 32) >> 6;
    px[i * 4 + 3] = ((64 - wA) * ep[e0 + 3] + wA * ep[e1 + 3] + 32) >> 6;
    if (rot) { const t = px[i * 4 + 3]; px[i * 4 + 3] = px[i * 4 + rot - 1]; px[i * 4 + rot - 1] = t; }
  }
}

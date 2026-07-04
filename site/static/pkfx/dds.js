// Minimal DDS decoder -> RGBA8. Handles the formats present in the corpus:
// BC1/DXT1, BC2/DXT3, BC3/DXT5 (the vast majority) and uncompressed 24/32-bit.
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

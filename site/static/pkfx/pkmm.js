// PopcornFX v1 .pkmm mesh decoder, following the engine's own reader
// (CResourceMeshFileSerializer_V01: file 0x180658df0, submesh 0x18066e530, vertex
// streams 0x1806737e0 in HH-Bridge_r.dll). Nothing is aligned; numbers are little-endian.
//
//   [u8 version][pad3][u32 nStrings][nStrings x (varlen size, bytes)]
//   chunks [u8 key][pad3][u32 size] until key 0; key 1 is a submesh (7 and 8 are skipped)
//   submesh: [flags][flags2][pad2][u32 material][u32 nSub][u32 nIndices]
//            (+48 bytes when flags2 & 1, else +16 when flags & 0x80), indices (size flags & 3),
//            then nSub sub-chunks [u8 key][pad3][u32 size]; sub-chunk 2 holds the vertex streams
//   streams: [u32 bytes][u32 V][u32 nStreams][u8 flags][pad3](+16 when flags & 2), one u32 code
//            per stream (code >> 8 = its name in the string table, low byte = format), then the
//            streams one after another
//
// Older bakers leave the pad bytes uninitialised, so the submesh header size is the one
// whose sub-chunks exactly fill the chunk. Returns null for a file this cannot read,
// and {empty: true, blocks: []} for one with no submesh (Trove ships a 40-byte stub).
// Otherwise every submesh merged for rendering, plus `blocks`, one per submesh, for
// shape samplers and SubMeshId.

// bytes per element for the stream format's low 5 bits
const ELEM = [1, 2, 3, 4, 1, 2, 3, 4, 2, 4, 6, 8, 2, 4, 6, 8, 4, 8, 12, 16, 4, 8, 12, 16, 2, 4, 6, 8, 4, 8, 12, 16];
const SLOT = { Position: 'positions', Normal: 'normals', Texcoord: 'uvs' };

export function decodePkmm(arrayBuffer) {
  const blocks = readBlocks(arrayBuffer);
  if (!blocks) return null;
  if (!blocks.length) return { empty: true, blocks };

  let vTotal = 0, iTotal = 0;
  for (const b of blocks) { vTotal += b.V; iTotal += b.indices.length; }
  const positions = new Float32Array(vTotal * 3);
  const normals = new Float32Array(vTotal * 3);
  const uvs = new Float32Array(vTotal * 2);
  const indices = vTotal > 65535 ? new Uint32Array(iTotal) : new Uint16Array(iTotal);
  let vo = 0, io = 0;
  for (const b of blocks) {
    positions.set(b.positions, vo * 3);
    if (b.normals) normals.set(b.normals, vo * 3);
    if (b.uvs) uvs.set(b.uvs, vo * 2);
    for (let i = 0; i < b.indices.length; i++) indices[io + i] = b.indices[i] + vo;
    vo += b.V; io += b.indices.length;
  }
  const bmin = [Infinity, Infinity, Infinity], bmax = [-Infinity, -Infinity, -Infinity];
  for (let i = 0; i < vTotal; i++) for (let k = 0; k < 3; k++) {
    const v = positions[i * 3 + k];
    if (v < bmin[k]) bmin[k] = v; if (v > bmax[k]) bmax[k] = v;
  }
  return { positions, normals, uvs, indices, vertexCount: vTotal, bmin, bmax, blocks };
}

function readBlocks(arrayBuffer) {
  const b = new Uint8Array(arrayBuffer), dv = new DataView(arrayBuffer);
  const u32 = (p) => dv.getUint32(p, true);
  try {
    const names = [];
    let p = 8;
    for (let i = u32(4); i > 0; i--) {
      // size prefix: the top 3 bits of its first byte give 0-3 more big-endian bytes
      const ext = [0, 0, 0, 0, 1, 1, 2, 3][b[p] >> 5];
      let n = b[p] & (0xff >> ext);
      for (let k = 1; k <= ext; k++) n = n * 256 + b[p + k];
      p += 1 + ext;
      names.push(new TextDecoder().decode(b.subarray(p, p + n)));
      if ((p += n) > b.length || names.length > 4096) return null;
    }
    const blocks = [];
    for (;;) {
      const key = b[p], size = u32(p + 4);
      p += 8;
      if (key === 0) break;
      if (p + size > b.length) return null;
      if (key === 1) {
        const blk = submesh(b, dv, p, p + size, names);
        if (!blk) return null;
        blocks.push(blk);
      }
      p += size;
    }
    return blocks;
  } catch {
    return null;   // ran off the end: not a mesh this reader knows
  }
}

function submesh(b, dv, q, end, names) {
  const u32 = (p) => dv.getUint32(p, true);
  const fl = b[q], isz = [1, 2, 4, 0][fl & 3], nSub = u32(q + 8), nIdx = u32(q + 12);
  if (!isz) return null;
  const x = fl & 0x80 ? 16 : 0;
  for (const ext of new Set([b[q + 1] & 1 ? 48 : x, x, 0])) {
    const idx = q + 16 + ext;
    let r = idx + nIdx * isz, vs = -1, s = 0;
    for (; s < nSub && r + 8 <= end; s++) {
      if (b[r] === 2 && vs < 0) vs = r + 8;
      r += 8 + u32(r + 4);
    }
    if (r !== end || s !== nSub || vs < 0) continue;
    const V = u32(vs + 4), nS = u32(vs + 8);
    let d = vs + 16 + (b[vs + 12] & 2 ? 16 : 0), data = d + nS * 4;
    const out = { V, positions: null, normals: null, uvs: null, indices: null };
    for (let j = 0; j < nS; j++, d += 4) {
      const code = u32(d), fmt = code & 0xff;
      // bit 0x80 pads a 12-byte element to 16
      const el = fmt & 0x80 && ELEM[fmt & 0x1f] === 12 ? 16 : ELEM[fmt & 0x1f];
      if (data + el * V > end) return null;
      const slot = SLOT[names[code >> 8]];
      if (slot && ((fmt >> 2) & 7) !== 7) return null;   // half or quantized floats: no Trove mesh uses them
      if (slot && !out[slot]) {
        const n = slot === 'uvs' ? 2 : 3, a = new Float32Array(V * n);
        for (let i = 0; i < V; i++) for (let k = 0; k < n; k++) a[i * n + k] = dv.getFloat32(data + i * el + k * 4, true);
        out[slot] = a;
      }
      data += el * V;
    }
    if (!out.positions) return null;
    const ind = (out.indices = new (V > 65535 ? Uint32Array : Uint16Array)(nIdx));
    for (let i = 0; i < nIdx; i++) {
      const v = isz === 1 ? b[idx + i] : isz === 2 ? dv.getUint16(idx + i * 2, true) : u32(idx + i * 4);
      if (v >= V) return null;
      ind[i] = v;
    }
    return out;
  }
  return null;
}

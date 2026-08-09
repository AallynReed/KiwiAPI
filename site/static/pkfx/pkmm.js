// PopcornFX v1 .pkmm mesh decoder (reverse-engineered against Trove's VFX library;
// 63/66 referenced meshes decode — the rest fall back to the box proxy).
//
// Layout: [u32 0][u32 nStrings][len-prefixed strings], then one block per submesh:
//   [u32 tag]  0x80 always set; 0x100 -> header carries s0,s1 + bbox floats
//   [u32 0][u32 ?][u32 indexCount]
//   [f32 s0,s1] [f32 bboxMin xyz, bboxMax xyz]?   (bbox iff tag & 0x100)
//   [u32 2][u32 0 x3]
//   index data — u8 / u16 / u32 chosen by vertex count (validated via the table below)
//   [u32 nRec][32-byte records]  rec0 = { bytes+28, V*bpv, V, attrCount, 0, 670, 990, 1053 }
//   f32x4 positions[V] · f32x4 normals[V] (bpv>=32) · f32x2 uvs[V] (bpv%16==8)
// Submesh blocks repeat; all are merged for rendering.

export function decodePkmm(arrayBuffer) {
  const buf = new Uint8Array(arrayBuffer);
  const dv = new DataView(arrayBuffer);
  const u32 = (p) => dv.getUint32(p, true);

  const blocks = [];
  let scan = 8;
  while (scan + 96 < buf.length) {
    const tag = u32(scan);
    if ((tag & 0x80) && tag < 1024 && u32(scan + 4) === 0) {
      const N = u32(scan + 12);
      if (N >= 3 && N <= 10_000_000 && N % 3 === 0) {
        const b = tryBlock(buf, dv, scan, N);
        if (b) { blocks.push(b); scan = b.end; continue; }
      }
    }
    scan += 1;
  }
  if (!blocks.length) return null;

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
  return { positions, normals, uvs, indices, vertexCount: vTotal, bmin, bmax };
}

function tryBlock(buf, dv, T, N) {
  const u32 = (p) => dv.getUint32(p, true);
  const u16 = (p) => dv.getUint16(p, true);
  const f32 = (p) => dv.getFloat32(p, true);
  for (const [idxStart, esz] of [[T + 64, 2], [T + 64, 1], [T + 64, 4], [T + 32, 2], [T + 32, 1], [T + 32, 4]]) {
    const idxEnd = idxStart + N * esz;
    if (idxEnd + 4 + 32 > buf.length) continue;
    const nRec = u32(idxEnd);
    if (nRec < 1 || nRec > 8) continue;
    const rec0 = idxEnd + 4;
    const V = u32(rec0 + 8);
    if (V < 3 || V > 2_000_000) continue;
    const bpv = u32(rec0 + 4) / V;                        // bytes per vertex, from the table itself
    if (bpv !== 24 && bpv !== 32 && bpv !== 40) continue; // pos4 [+nrm4] [+uv2]
    if (esz === 1 && V > 256) continue;
    if (esz === 2 && V > 65536) continue;
    const hasNrm = bpv >= 32, hasUv = bpv % 16 === 8;
    const rdIdx = (i) => esz === 1 ? buf[idxStart + i] : esz === 2 ? u16(idxStart + i * 2) : u32(idxStart + i * 4);
    let maxIdx = 0;
    for (let i = 0; i < N; i++) { const v = rdIdx(i); if (v > maxIdx) maxIdx = v; }
    if (maxIdx >= V) continue;
    const vtxStart = idxEnd + 4 + nRec * 32;
    const posEnd = vtxStart + V * 16;
    const nrmEnd = posEnd + (hasNrm ? V * 16 : 0);
    const uvEnd = nrmEnd + (hasUv ? V * 8 : 0);
    if (uvEnd > buf.length) continue;
    const positions = new Float32Array(V * 3);
    const normals = hasNrm ? new Float32Array(V * 3) : null;
    const uvs = hasUv ? new Float32Array(V * 2) : null;
    let bad = false;
    for (let i = 0; i < V; i++) {
      for (let k = 0; k < 3; k++) {
        const v = f32(vtxStart + i * 16 + k * 4);
        if (!isFinite(v) || Math.abs(v) > 1e6) { bad = true; break; }
        positions[i * 3 + k] = v;
      }
      if (bad) break;
      if (hasNrm) for (let k = 0; k < 3; k++) normals[i * 3 + k] = f32(posEnd + i * 16 + k * 4);
      if (hasUv) { uvs[i * 2] = f32(nrmEnd + i * 8); uvs[i * 2 + 1] = f32(nrmEnd + i * 8 + 4); }
    }
    if (bad) continue;
    const indices = new (V > 65535 ? Uint32Array : Uint16Array)(N);
    for (let i = 0; i < N; i++) indices[i] = rdIdx(i);
    return { V, indices, positions, normals, uvs, end: uvEnd };
  }
  return null;
}

/* Kiwi voxel mesher - turns a voxel payload into drawable geometry.

   Both 3D viewers used to draw every voxel as a complete 1x1x1 cube. For solids
   that only wastes triangles (you can't see an interior face through an opaque
   one), but for GLASS it's visible: the faces where two transparent voxels touch
   are still drawn, so a pane of glass shows a grid of internal quads and reads as
   darker where voxels overlap. This emits only the faces that can actually be
   seen, by the same rule the server's catalog rasterizer uses
   (`app/trove/render/voxel.py::_faces`), so the WebGL preview and the PNG render
   finally agree:

     - no neighbour                      -> draw
     - opaque, neighbour opaque          -> cull (never visible)
     - opaque, neighbour transparent     -> draw (the solid shows THROUGH the glass)
     - transparent, ANY neighbour        -> cull (this is the fix)

   Fewer faces is also just less geometry: a solid model drops its whole interior,
   and nothing has to build a per-voxel instance matrix any more.

   Input is one voxel set - a single blueprint, or one part of an assembled
   creature - as {x, y, z, rgb, kind, level}, plain arrays or typed arrays alike.
   Output is one mesh per material group (kind + glass level), coloured per vertex. */
(function () {
  'use strict';

  // Per face: outward normal, then the two tangents u,v with u x v = n, so the
  // quad p0..p3 below winds counter-clockwise seen from outside.
  var FACES = [
    [1, 0, 0, 0, 1, 0, 0, 0, 1],
    [-1, 0, 0, 0, 0, 1, 0, 1, 0],
    [0, 1, 0, 0, 0, 1, 1, 0, 0],
    [0, -1, 0, 1, 0, 0, 0, 0, 1],
    [0, 0, 1, 1, 0, 0, 0, 1, 0],
    [0, 0, -1, 0, 1, 0, 1, 0, 0]
  ];

  // Beyond this the bounding box is mostly empty air, so index by key instead of
  // paying for the grid (a 64 MB Int32Array for a handful of voxels).
  var MAX_CELLS = 16 << 20;

  function isOpaque(kind) { return kind === 0 || kind === 2; }   // solid / glow

  /* 0 solid · 1 glass · 2 glow · 3 glow-glass. Glass opacity = (level/255)^2
     (level = 16+32*w), matching the game/catalog. Colour rides on the vertices. */
  function makeMaterial(THREE, kind, level) {
    var opacity = Math.pow((level || 255) / 255, 2);
    if (kind === 2) return new THREE.MeshBasicMaterial({ vertexColors: true });
    if (kind === 3) return new THREE.MeshBasicMaterial({ vertexColors: true, transparent: true, opacity: opacity, depthWrite: false });
    if (kind === 1) return new THREE.MeshPhongMaterial({ vertexColors: true, transparent: true, opacity: opacity, depthWrite: false, shininess: 70, specular: 0x4d4d4d });
    return new THREE.MeshPhongMaterial({ vertexColors: true, shininess: 28, specular: 0x1c1c1c });
  }

  /* Which voxel (if any) sits at each cell, so a face can ask its neighbour's kind. */
  function occupancy(X, Y, Z, n) {
    var minX = Infinity, minY = Infinity, minZ = Infinity;
    var maxX = -Infinity, maxY = -Infinity, maxZ = -Infinity, i;
    for (i = 0; i < n; i++) {
      if (X[i] < minX) minX = X[i]; if (X[i] > maxX) maxX = X[i];
      if (Y[i] < minY) minY = Y[i]; if (Y[i] > maxY) maxY = Y[i];
      if (Z[i] < minZ) minZ = Z[i]; if (Z[i] > maxZ) maxZ = Z[i];
    }
    var w = maxX - minX + 3, h = maxY - minY + 3, d = maxZ - minZ + 3;   // 1 cell of padding
    if (n && w * h * d <= MAX_CELLS) {
      var grid = new Int32Array(w * h * d), hd = h * d;
      for (i = 0; i < n; i++) grid[(X[i] - minX + 1) * hd + (Y[i] - minY + 1) * d + (Z[i] - minZ + 1)] = i + 1;
      return function (x, y, z) {
        x -= minX; y -= minY; z -= minZ;
        if (x < -1 || y < -1 || z < -1 || x > w - 2 || y > h - 2 || z > d - 2) return 0;
        return grid[(x + 1) * hd + (y + 1) * d + (z + 1)];
      };
    }
    var map = new Map();
    for (i = 0; i < n; i++) map.set(X[i] + ',' + Y[i] + ',' + Z[i], i + 1);
    return function (x, y, z) { return map.get(x + ',' + y + ',' + z) || 0; };
  }

  function build(THREE, part) {
    var X = part.x, Y = part.y, Z = part.z, RGB = part.rgb;
    var KIND = part.kind, LVL = part.level;
    var n = X ? X.length : 0;
    if (!n) return [];

    var at = occupancy(X, Y, Z, n);
    var kindOf = function (i) { return KIND ? (KIND[i] || 0) : 0; };
    var groupOf = function (i, k) {
      return k + ':' + ((k === 1 || k === 3) ? (LVL ? LVL[i] : 255) : 255);
    };

    // Pass 1: decide every face once, remember the six-bit answer, and count how
    // much geometry each material group needs.
    var mask = new Uint8Array(n), counts = {}, i, f;
    for (i = 0; i < n; i++) {
      var k = kindOf(i), opaque = isOpaque(k), bits = 0, drawn = 0;
      for (f = 0; f < 6; f++) {
        var F = FACES[f], nb = at(X[i] + F[0], Y[i] + F[1], Z[i] + F[2]);
        var draw = nb === 0 ? true : (opaque && !isOpaque(kindOf(nb - 1)));
        if (draw) { bits |= 1 << f; drawn++; }
      }
      mask[i] = bits;
      if (drawn) { var g = groupOf(i, k); counts[g] = (counts[g] || 0) + drawn; }
    }

    var buffers = {};
    Object.keys(counts).forEach(function (g) {
      var q = counts[g];
      buffers[g] = { pos: new Float32Array(q * 12), nor: new Float32Array(q * 12),
                     col: new Float32Array(q * 12), idx: new Uint32Array(q * 6), v: 0, t: 0 };
    });

    // Pass 2: write the quads.
    for (i = 0; i < n; i++) {
      if (!mask[i]) continue;
      var b = buffers[groupOf(i, kindOf(i))];
      var c = RGB ? RGB[i] : 0xffffff;
      var cr = ((c >> 16) & 255) / 255, cg = ((c >> 8) & 255) / 255, cb = (c & 255) / 255;
      var x = X[i], y = Y[i], z = Z[i];
      for (f = 0; f < 6; f++) {
        if (!(mask[i] & (1 << f))) continue;
        var F = FACES[f];
        var fx = x + F[0] * 0.5, fy = y + F[1] * 0.5, fz = z + F[2] * 0.5;   // face centre
        var ux = F[3] * 0.5, uy = F[4] * 0.5, uz = F[5] * 0.5;
        var vx = F[6] * 0.5, vy = F[7] * 0.5, vz = F[8] * 0.5;
        var base = b.v / 3, p = b.v;
        // p0 = c-u-v, p1 = c+u-v, p2 = c+u+v, p3 = c-u+v
        b.pos[p] = fx - ux - vx; b.pos[p + 1] = fy - uy - vy; b.pos[p + 2] = fz - uz - vz;
        b.pos[p + 3] = fx + ux - vx; b.pos[p + 4] = fy + uy - vy; b.pos[p + 5] = fz + uz - vz;
        b.pos[p + 6] = fx + ux + vx; b.pos[p + 7] = fy + uy + vy; b.pos[p + 8] = fz + uz + vz;
        b.pos[p + 9] = fx - ux + vx; b.pos[p + 10] = fy - uy + vy; b.pos[p + 11] = fz - uz + vz;
        for (var j = 0; j < 4; j++) {
          b.nor[p + j * 3] = F[0]; b.nor[p + j * 3 + 1] = F[1]; b.nor[p + j * 3 + 2] = F[2];
          b.col[p + j * 3] = cr; b.col[p + j * 3 + 1] = cg; b.col[p + j * 3 + 2] = cb;
        }
        b.v += 12;
        b.idx[b.t] = base; b.idx[b.t + 1] = base + 1; b.idx[b.t + 2] = base + 2;
        b.idx[b.t + 3] = base; b.idx[b.t + 4] = base + 2; b.idx[b.t + 5] = base + 3;
        b.t += 6;
      }
    }

    return Object.keys(buffers).map(function (g) {
      var b = buffers[g], kv = g.split(':'), k = +kv[0], lv = +kv[1];
      var geo = new THREE.BufferGeometry();
      geo.setAttribute('position', new THREE.BufferAttribute(b.pos, 3));
      geo.setAttribute('normal', new THREE.BufferAttribute(b.nor, 3));
      geo.setAttribute('color', new THREE.BufferAttribute(b.col, 3));
      geo.setIndex(new THREE.BufferAttribute(b.idx, 1));
      geo.computeBoundingSphere();
      var mesh = new THREE.Mesh(geo, makeMaterial(THREE, k, lv));
      if (k === 1 || k === 3) mesh.renderOrder = 1;      // draw transparent after opaque
      return mesh;
    });
  }

  window.VoxelMesh = { build: build, makeMaterial: makeMaterial };
})();

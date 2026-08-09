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
   creature - as {x, y, z, rgb, kind, level, spec}, plain arrays or typed arrays
   alike. Output is one mesh per material group (kind + glass level + specular),
   coloured per vertex. */
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

  /* --- The specular map -----------------------------------------------------

     A solid voxel carries a specular value (rough · metal · water · iridescent ·
     waxy) that the previews used to ignore, so a knight's steel and a gold badge
     shaded exactly like painted wood. Trove doesn't derive a highlight from a
     roughness number: the value indexes a 4x2 atlas of pre-baked lobes
     (textures/brdfmap.dds) that its shader samples by (N·H, L·H) - the rainbow
     sheen of "iridescent" is literally a tile of that image. So we sample the
     same atlas the same way rather than inventing per-material shininess:

       Lighting_BRDFSpecular(), programs/fragment/library_specularlighting.fragment

     The lobe multiplies the voxel's own colour (the game's tintColor), which is
     why gold shimmers gold instead of white. Until the atlas arrives - or if the
     server can't serve it - the uniform holds an empty texture, which samples
     black, so every solid falls back to a flat diffuse shade. */
  var BRDF_MAP = { value: null };          // shared by every material; filled once
  var BRDF_LIGHT = { value: null };        // key-light direction, world space
  // The game feeds its lobes a tuned sunLightSpecular; the viewers' own rig is
  // already bright, so the strongest lobe (metal, near-white) is scaled to add
  // about a third of the voxel's colour instead of doubling it and clipping.
  var BRDF_GAIN = { value: 0.35 };
  var _brdfLoad = null, _brdfWaiters = [];

  var BRDF_UNIFORMS =
    'uniform sampler2D kBrdfMap;\nuniform vec3 kBrdfLight;\nuniform float kBrdfGain;\n' +
    'uniform float kBrdfTile;\n';

  // index.y picks the atlas row (tiles 0-3 bottom, 4-7 top); the clamps are the
  // game's own half-texel inset, which keeps a lobe from bleeding into its neighbour.
  var BRDF_LOBE = [
    'vec3 kN = normalize(vNormal);',
    'vec3 kE = normalize(vViewPosition);',
    'vec3 kL = normalize((viewMatrix * vec4(kBrdfLight, 0.0)).xyz);',
    'vec3 kH = normalize(kE + kL);',
    'vec2 kTx = vec2(clamp(dot(kN, kH), 0.0078125, 0.9921875),',
    '                clamp(dot(kL, kH), 0.00390625, 0.99609375));',
    'vec2 kCell = vec2(mod(kBrdfTile, 4.0), kBrdfTile < 4.0 ? 1.0 : 0.0);',
    'vec3 kLobe = texture2D(kBrdfMap, (kCell + kTx) * vec2(0.25, 0.5)).rgb;',
    'outgoingLight += diffuseColor.rgb * kLobe *',
    '                 clamp(dot(kN, kL) * 2.0 + 1.0, 0.0, 1.0) * kBrdfGain;',
    '#include <output_fragment>'
  ].join('\n');

  /* Fetch the atlas once per page. Failure is not an error - the model still
     draws, just without highlights - so a missing texture costs fidelity, not a model. */
  function loadBrdf(THREE, url, onReady) {
    if (BRDF_MAP.value) return;
    if (onReady) _brdfWaiters.push(onReady);
    if (_brdfLoad) return;
    _brdfLoad = new THREE.TextureLoader().load(url, function (tex) {
      tex.flipY = false;                   // atlas rows are top-down, as the game samples them
      tex.wrapS = tex.wrapT = THREE.ClampToEdgeWrapping;
      tex.minFilter = tex.magFilter = THREE.LinearFilter;
      tex.generateMipmaps = false;         // a mip would smear one lobe into the next
      BRDF_MAP.value = tex;
      _brdfWaiters.splice(0).forEach(function (fn) { fn(); });
    }, undefined, function () { _brdfWaiters.length = 0; });
  }

  /* 0 solid · 1 glass · 2 glow · 3 glow-glass. Glass opacity = (level/255)^2
     (level = 16+32*w), matching the game/catalog. Colour rides on the vertices.
     Solids take their highlight from the specular atlas above, so their own Phong
     specular is off - one lighting model, not two fighting each other. */
  function makeMaterial(THREE, kind, level, spec) {
    var opacity = Math.pow((level || 255) / 255, 2);
    if (kind === 2) return new THREE.MeshBasicMaterial({ vertexColors: true });
    if (kind === 3) return new THREE.MeshBasicMaterial({ vertexColors: true, transparent: true, opacity: opacity, depthWrite: false });
    if (kind === 1) return new THREE.MeshPhongMaterial({ vertexColors: true, transparent: true, opacity: opacity, depthWrite: false, shininess: 70, specular: 0x4d4d4d });

    var mat = new THREE.MeshPhongMaterial({ vertexColors: true, shininess: 0, specular: 0x000000 });
    if (!BRDF_LIGHT.value) BRDF_LIGHT.value = new THREE.Vector3(0.7, 1.0, 0.55);
    var tile = { value: spec || 0 };
    mat.onBeforeCompile = function (shader) {
      shader.uniforms.kBrdfMap = BRDF_MAP;
      shader.uniforms.kBrdfLight = BRDF_LIGHT;
      shader.uniforms.kBrdfGain = BRDF_GAIN;
      shader.uniforms.kBrdfTile = tile;
      shader.fragmentShader = shader.fragmentShader
        .replace('#include <common>', '#include <common>\n' + BRDF_UNIFORMS)
        .replace('#include <output_fragment>', BRDF_LOBE);
    };
    mat.customProgramCacheKey = function () { return 'kbrdf'; };
    return mat;
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

  /* `opts`: { brdfUrl, lightDir:[x,y,z], onReady } - where to fetch the specular
     atlas, which light its lobes answer to, and a redraw hook (the viewers render
     on demand, so a texture that lands after the first frame needs one). */
  function build(THREE, part, opts) {
    var X = part.x, Y = part.y, Z = part.z, RGB = part.rgb;
    var KIND = part.kind, LVL = part.level, SPEC = part.spec;
    var n = X ? X.length : 0;
    if (!n) return [];

    opts = opts || {};
    if (opts.lightDir) {
      BRDF_LIGHT.value = new THREE.Vector3(opts.lightDir[0], opts.lightDir[1], opts.lightDir[2]);
    }
    if (opts.brdfUrl) loadBrdf(THREE, opts.brdfUrl, opts.onReady);

    var at = occupancy(X, Y, Z, n);
    var kindOf = function (i) { return KIND ? (KIND[i] || 0) : 0; };
    var groupOf = function (i, k) {
      return k + ':' + ((k === 1 || k === 3) ? (LVL ? LVL[i] : 255) : 255)
               + ':' + (k === 0 && SPEC ? (SPEC[i] || 0) : 0);
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
      var b = buffers[g], kv = g.split(':'), k = +kv[0], lv = +kv[1], sp = +kv[2];
      var geo = new THREE.BufferGeometry();
      geo.setAttribute('position', new THREE.BufferAttribute(b.pos, 3));
      geo.setAttribute('normal', new THREE.BufferAttribute(b.nor, 3));
      geo.setAttribute('color', new THREE.BufferAttribute(b.col, 3));
      geo.setIndex(new THREE.BufferAttribute(b.idx, 1));
      geo.computeBoundingSphere();
      var mesh = new THREE.Mesh(geo, makeMaterial(THREE, k, lv, sp));
      if (k === 1 || k === 3) mesh.renderOrder = 1;      // draw transparent after opaque
      return mesh;
    });
  }

  window.VoxelMesh = { build: build, makeMaterial: makeMaterial };
})();

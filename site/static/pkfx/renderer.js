// WebGL2 renderer for the particle simulation: instanced billboards (all
// PopcornFX billboarding modes), ribbons, and instanced real meshes.
import { perspective, lookAt } from './glmath.js';

// Per-instance layout for billboards:
//   center3, size2, color4, rot1, uvrect4, uvrect2_4, blend1, axis3, axis2_3, cursor1
export const FLOATS_PER_INSTANCE = 26;

const VERT = `#version 300 es
precision highp float;
layout(location=0) in vec2 aCorner;   // -0.5..0.5 quad corner
layout(location=1) in vec2 aUV;       // 0..1
layout(location=2) in vec3 aCenter;
layout(location=3) in vec2 aSize;
layout(location=4) in vec4 aColor;
layout(location=5) in float aRot;
layout(location=6) in vec4 aUVRect;   // u0,v0,du,dv
layout(location=7) in vec4 aUVRect2;  // next flipbook frame (soft anim blending)
layout(location=8) in float aBlend;   // frame blend weight
layout(location=9) in vec3 aAxis;     // stretch axis (velocity*AxisScale) / planar axis
layout(location=10) in vec3 aAxis2;   // planar normal
layout(location=11) in float aCursor; // alpha-remap cursor
uniform mat4 uView, uProj;
uniform vec3 uEye;
uniform int uMode;  // 0 screen, 1 viewpos, 2 axis, 3 spheroid, 4 planar
out vec2 vUV; out vec2 vUV2; out vec4 vColor; out float vBlend; out float vCursor;
void main(){
  float s = sin(aRot), c = cos(aRot);
  vec2 rot = vec2(aCorner.x*c - aCorner.y*s, aCorner.x*s + aCorner.y*c);
  vec3 world;
  if (uMode == 2 || uMode == 3) {
    float L = length(aAxis);
    vec3 toEye = normalize(uEye - aCenter);
    vec3 dir = L > 1e-5 ? aAxis / L : vec3(uView[0][1], uView[1][1], uView[2][1]);
    vec3 side = cross(dir, toEye);
    float sl = length(side);
    side = sl > 1e-5 ? side / sl : vec3(uView[0][0], uView[1][0], uView[2][0]);
    float halfLen = 0.5*L + 0.5*aSize.y;
    world = aCenter + side*(aCorner.x*aSize.x) + dir*(aCorner.y*2.0*halfLen);
  } else if (uMode == 4) {
    vec3 n = length(aAxis2) > 1e-5 ? normalize(aAxis2) : vec3(0.0, 1.0, 0.0);
    vec3 t = aAxis - n*dot(aAxis, n);
    vec3 T = length(t) > 1e-5 ? normalize(t)
           : normalize(abs(n.y) < 0.99 ? cross(n, vec3(0.0, 1.0, 0.0)) : vec3(1.0, 0.0, 0.0));
    vec3 B = cross(n, T);
    // rotate the in-plane basis around the normal
    vec3 Tr = T*c + B*s, Br = -T*s + B*c;
    world = aCenter + Tr*(aCorner.x*aSize.x) + Br*(aCorner.y*aSize.y);
  } else if (uMode == 1) {
    vec3 fwd = normalize(uEye - aCenter);
    vec3 right = normalize(cross(vec3(0.0, 1.0, 0.0), fwd));
    if (length(cross(vec3(0.0,1.0,0.0), fwd)) < 1e-4) right = vec3(1.0, 0.0, 0.0);
    vec3 up = cross(fwd, right);
    world = aCenter + right*(rot.x*aSize.x) + up*(rot.y*aSize.y);
  } else {
    vec3 right = vec3(uView[0][0], uView[1][0], uView[2][0]);
    vec3 up    = vec3(uView[0][1], uView[1][1], uView[2][1]);
    world = aCenter + right*(rot.x*aSize.x) + up*(rot.y*aSize.y);
  }
  gl_Position = uProj * uView * vec4(world, 1.0);
  vUV = aUVRect.xy + aUV * aUVRect.zw;
  vUV2 = aUVRect2.xy + aUV * aUVRect2.zw;
  vColor = aColor; vBlend = aBlend; vCursor = aCursor;
}`;

const FRAG = `#version 300 es
precision highp float;
in vec2 vUV; in vec2 vUV2; in vec4 vColor; in float vBlend; in float vCursor;
uniform sampler2D uTex;
uniform sampler2D uRemap;
uniform int uHasRemap;
uniform int uKind;   // 0 alpha, 1 additive, 2 alphablend_additive, 3 additive_noalpha
out vec4 frag;
void main(){
  vec4 t = mix(texture(uTex, vUV), texture(uTex, vUV2), vBlend);
  vec4 c = t * vColor;
  if (uHasRemap == 1) {
    // remap ramp lives in the R channel: out_alpha = remap(in_alpha, cursor)
    c.a = texture(uRemap, vec2(clamp(t.a, 0.0, 1.0), clamp(vCursor, 0.0, 1.0))).r * vColor.a;
  }
  if (uKind == 3) { frag = vec4(t.rgb * vColor.rgb * vColor.a, 1.0); return; }
  if (uKind == 2) { if (c.a < 0.003 && dot(c.rgb, vec3(1.0)) < 0.01) discard; frag = vec4(c.rgb * vColor.a, c.a); return; }
  if (c.a < 0.003) discard;
  frag = c;
}`;

// Ribbon program: generic textured triangles in world space.
const RVERT = `#version 300 es
precision highp float;
layout(location=0) in vec3 aPos;
layout(location=1) in vec2 aUV;
layout(location=2) in vec4 aColor;
uniform mat4 uView, uProj;
out vec2 vUV; out vec4 vColor;
void main(){ gl_Position = uProj*uView*vec4(aPos,1.0); vUV=aUV; vColor=aColor; }`;

const RFRAG = `#version 300 es
precision highp float;
in vec2 vUV; in vec4 vColor; uniform sampler2D uTex; out vec4 frag;
void main(){ vec4 c = texture(uTex,vUV)*vColor; if(c.a<0.003) discard; frag=c; }`;

export const RIBBON_FLOATS_PER_VERT = 9; // pos3, uv2, color4

// Mesh program: instanced textured geometry with a per-instance basis (orientation*scale).
const MVERT = `#version 300 es
precision highp float;
layout(location=0) in vec3 aPos;
layout(location=1) in vec3 aNormal;
layout(location=2) in vec2 aUV;
layout(location=3) in vec3 aBX;      // basis columns (rotation * scale)
layout(location=4) in vec3 aBY;
layout(location=5) in vec3 aBZ;
layout(location=6) in vec3 aCenter;
layout(location=7) in vec4 aColor;
uniform mat4 uView, uProj;
out vec3 vN; out vec2 vUV; out vec4 vColor;
void main(){
  mat3 B = mat3(aBX, aBY, aBZ);
  vec3 world = aCenter + B * aPos;
  gl_Position = uProj*uView*vec4(world,1.0);
  vN = B * aNormal; vUV = aUV; vColor = aColor;
}`;
const MFRAG = `#version 300 es
precision highp float;
in vec3 vN; in vec2 vUV; in vec4 vColor;
uniform sampler2D uTex;
uniform int uLit;      // Solid -> lit + alpha cutout; additive -> unlit
out vec4 frag;
void main(){
  vec4 t = texture(uTex, vUV);
  vec4 c = t * vColor;
  if (uLit == 1) {
    if (c.a < 0.5) discard;
    vec3 n = normalize(vN);
    float l = max(dot(n, normalize(vec3(0.4,0.85,0.5))),0.0)*0.6 + 0.45;
    frag = vec4(c.rgb*l, 1.0);
  } else {
    if (c.a < 0.003 && dot(c.rgb, vec3(1.0)) < 0.01) discard;
    frag = c;
  }
}`;
export const MESH_FLOATS_PER_INSTANCE = 16; // basis 9, center 3, color 4

const QUAD = new Float32Array([
  // corner.xy, uv.xy
  -0.5, -0.5, 0, 1,
   0.5, -0.5, 1, 1,
  -0.5,  0.5, 0, 0,
   0.5,  0.5, 1, 0,
]);

export class Renderer {
  constructor(canvas) {
    const gl = canvas.getContext('webgl2', { alpha: true, premultipliedAlpha: false, antialias: true });
    if (!gl) throw new Error('WebGL2 not available');
    this.gl = gl; this.canvas = canvas;

    // billboard program
    this.prog = makeProgram(gl, VERT, FRAG);
    const u = (n) => gl.getUniformLocation(this.prog, n);
    this.u = { view: u('uView'), proj: u('uProj'), eye: u('uEye'), mode: u('uMode'), tex: u('uTex'), remap: u('uRemap'), hasRemap: u('uHasRemap'), kind: u('uKind') };
    this.vao = gl.createVertexArray();
    gl.bindVertexArray(this.vao);
    this.quadBuf = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, this.quadBuf);
    gl.bufferData(gl.ARRAY_BUFFER, QUAD, gl.STATIC_DRAW);
    gl.enableVertexAttribArray(0); gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 16, 0);
    gl.enableVertexAttribArray(1); gl.vertexAttribPointer(1, 2, gl.FLOAT, false, 16, 8);
    this.instBuf = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, this.instBuf);
    const stride = FLOATS_PER_INSTANCE * 4;
    const setup = (loc, size, off) => { gl.enableVertexAttribArray(loc); gl.vertexAttribPointer(loc, size, gl.FLOAT, false, stride, off); gl.vertexAttribDivisor(loc, 1); };
    setup(2, 3, 0);        // center
    setup(3, 2, 12);       // size
    setup(4, 4, 20);       // color
    setup(5, 1, 36);       // rot
    setup(6, 4, 40);       // uvrect
    setup(7, 4, 56);       // uvrect2
    setup(8, 1, 72);       // blend
    setup(9, 3, 76);       // axis
    setup(10, 3, 88);      // axis2
    setup(11, 1, 100);     // cursor
    gl.bindVertexArray(null);

    // ribbon program + its own VAO/buffer
    this.rprog = makeProgram(gl, RVERT, RFRAG);
    this.ruView = gl.getUniformLocation(this.rprog, 'uView');
    this.ruProj = gl.getUniformLocation(this.rprog, 'uProj');
    this.ruTex = gl.getUniformLocation(this.rprog, 'uTex');
    this.rvao = gl.createVertexArray();
    gl.bindVertexArray(this.rvao);
    this.rbuf = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, this.rbuf);
    const rstride = RIBBON_FLOATS_PER_VERT * 4;
    gl.enableVertexAttribArray(0); gl.vertexAttribPointer(0, 3, gl.FLOAT, false, rstride, 0);
    gl.enableVertexAttribArray(1); gl.vertexAttribPointer(1, 2, gl.FLOAT, false, rstride, 12);
    gl.enableVertexAttribArray(2); gl.vertexAttribPointer(2, 4, gl.FLOAT, false, rstride, 20);
    gl.bindVertexArray(null);

    // mesh program (geometries are created per mesh via makeMeshGeometry)
    this.mprog = makeProgram(gl, MVERT, MFRAG);
    this.muView = gl.getUniformLocation(this.mprog, 'uView');
    this.muProj = gl.getUniformLocation(this.mprog, 'uProj');
    this.muTex = gl.getUniformLocation(this.mprog, 'uTex');
    this.muLit = gl.getUniformLocation(this.mprog, 'uLit');
    this.cubeGeom = this.makeMeshGeometry(buildCubeMesh());

    this.white = makeTexture(gl, 1, 1, new Uint8ClampedArray([255, 255, 255, 255]));
    this.cam = { az: 0.6, el: 0.3, dist: 14, target: [0, 1.5, 0] };
  }

  // Upload an indexed mesh {positions, normals, uvs, indices} -> instanced geometry.
  makeMeshGeometry(mesh) {
    const gl = this.gl;
    const vao = gl.createVertexArray();
    gl.bindVertexArray(vao);
    const n = mesh.positions.length / 3;
    const inter = new Float32Array(n * 8);
    for (let i = 0; i < n; i++) {
      inter[i * 8] = mesh.positions[i * 3]; inter[i * 8 + 1] = mesh.positions[i * 3 + 1]; inter[i * 8 + 2] = mesh.positions[i * 3 + 2];
      inter[i * 8 + 3] = mesh.normals ? mesh.normals[i * 3] : 0; inter[i * 8 + 4] = mesh.normals ? mesh.normals[i * 3 + 1] : 1; inter[i * 8 + 5] = mesh.normals ? mesh.normals[i * 3 + 2] : 0;
      inter[i * 8 + 6] = mesh.uvs ? mesh.uvs[i * 2] : 0; inter[i * 8 + 7] = mesh.uvs ? mesh.uvs[i * 2 + 1] : 0;
    }
    const vbuf = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, vbuf);
    gl.bufferData(gl.ARRAY_BUFFER, inter, gl.STATIC_DRAW);
    gl.enableVertexAttribArray(0); gl.vertexAttribPointer(0, 3, gl.FLOAT, false, 32, 0);
    gl.enableVertexAttribArray(1); gl.vertexAttribPointer(1, 3, gl.FLOAT, false, 32, 12);
    gl.enableVertexAttribArray(2); gl.vertexAttribPointer(2, 2, gl.FLOAT, false, 32, 24);
    const ibuf = gl.createBuffer();
    gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, ibuf);
    const idx32 = mesh.indices instanceof Uint32Array;
    gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, mesh.indices, gl.STATIC_DRAW);
    const instBuf = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, instBuf);
    const ms = MESH_FLOATS_PER_INSTANCE * 4;
    const msetup = (loc, size, off) => { gl.enableVertexAttribArray(loc); gl.vertexAttribPointer(loc, size, gl.FLOAT, false, ms, off); gl.vertexAttribDivisor(loc, 1); };
    msetup(3, 3, 0); msetup(4, 3, 12); msetup(5, 3, 24); msetup(6, 3, 36); msetup(7, 4, 48);
    gl.bindVertexArray(null);
    return { vao, instBuf, indexCount: mesh.indices.length, indexType: idx32 ? gl.UNSIGNED_INT : gl.UNSIGNED_SHORT };
  }

  resize() {
    const c = this.canvas; const dpr = Math.min(window.devicePixelRatio || 1, 2);
    const w = Math.floor(c.clientWidth * dpr), h = Math.floor(c.clientHeight * dpr);
    if (c.width !== w || c.height !== h) { c.width = w; c.height = h; }
  }

  eyePosition() {
    const { az, el, dist, target } = this.cam;
    return [target[0] + dist * Math.cos(el) * Math.sin(az), target[1] + dist * Math.sin(el), target[2] + dist * Math.cos(el) * Math.cos(az)];
  }

  _blend(kind) {
    const gl = this.gl;
    if (kind === 1) gl.blendFunc(gl.SRC_ALPHA, gl.ONE);
    else if (kind === 2) gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
    else if (kind === 3) gl.blendFunc(gl.ONE, gl.ONE);
    else gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);
  }

  // items: mixed draw list, each { type: 'billboard'|'ribbon'|'mesh', drawOrder, ... }
  //   billboard: { texture, remapTexture?, kind, mode, instances, count }
  //   ribbon:    { texture, kind, vertices, count }   (count = vertices)
  //   mesh:      { geom, texture, lit, kind, instances, count }
  draw(items) {
    const gl = this.gl; this.resize();
    gl.viewport(0, 0, this.canvas.width, this.canvas.height);
    gl.clearColor(0.05, 0.05, 0.07, 1); gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);
    gl.enable(gl.DEPTH_TEST);

    const aspect = this.canvas.width / this.canvas.height;
    const proj = perspective(60 * Math.PI / 180, aspect, 0.1, 1000);
    const eye = this.eyePosition();
    const view = lookAt(eye, this.cam.target, [0, 1, 0]);
    gl.activeTexture(gl.TEXTURE0);

    // opaque (lit solid) meshes first with depth write, then everything else sorted
    const solid = items.filter((d) => d.type === 'mesh' && d.lit);
    const trans = items.filter((d) => !(d.type === 'mesh' && d.lit)).sort((a, b) => (a.drawOrder || 0) - (b.drawOrder || 0));

    if (solid.length) {
      gl.depthMask(true); gl.disable(gl.BLEND);
      gl.useProgram(this.mprog);
      gl.uniformMatrix4fv(this.muView, false, view);
      gl.uniformMatrix4fv(this.muProj, false, proj);
      gl.uniform1i(this.muTex, 0);
      gl.uniform1i(this.muLit, 1);
      for (const d of solid) this._drawMesh(d);
    }

    gl.depthMask(false); gl.enable(gl.BLEND);
    let prog = null;
    for (const d of trans) {
      if (!d.count) continue;
      this._blend(d.kind || 0);
      if (d.type === 'billboard') {
        if (prog !== 'b') { prog = 'b';
          gl.useProgram(this.prog);
          gl.uniformMatrix4fv(this.u.view, false, view);
          gl.uniformMatrix4fv(this.u.proj, false, proj);
          gl.uniform3f(this.u.eye, eye[0], eye[1], eye[2]);
          gl.uniform1i(this.u.tex, 0);
          gl.uniform1i(this.u.remap, 1);
          gl.bindVertexArray(this.vao);
        }
        gl.uniform1i(this.u.mode, d.mode || 0);
        gl.uniform1i(this.u.kind, d.kind || 0);
        gl.uniform1i(this.u.hasRemap, d.remapTexture ? 1 : 0);
        gl.activeTexture(gl.TEXTURE1);
        gl.bindTexture(gl.TEXTURE_2D, d.remapTexture || this.white);
        gl.activeTexture(gl.TEXTURE0);
        gl.bindTexture(gl.TEXTURE_2D, d.texture || this.white);
        gl.bindBuffer(gl.ARRAY_BUFFER, this.instBuf);
        gl.bufferData(gl.ARRAY_BUFFER, d.instances.subarray(0, d.count * FLOATS_PER_INSTANCE), gl.DYNAMIC_DRAW);
        gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, d.count);
      } else if (d.type === 'ribbon') {
        if (prog !== 'r') { prog = 'r';
          gl.useProgram(this.rprog);
          gl.uniformMatrix4fv(this.ruView, false, view);
          gl.uniformMatrix4fv(this.ruProj, false, proj);
          gl.uniform1i(this.ruTex, 0);
          gl.bindVertexArray(this.rvao);
        }
        gl.bindTexture(gl.TEXTURE_2D, d.texture || this.white);
        gl.bindBuffer(gl.ARRAY_BUFFER, this.rbuf);
        gl.bufferData(gl.ARRAY_BUFFER, d.vertices.subarray(0, d.count * RIBBON_FLOATS_PER_VERT), gl.DYNAMIC_DRAW);
        gl.drawArrays(gl.TRIANGLES, 0, d.count);
      } else if (d.type === 'mesh') {
        prog = null;
        gl.useProgram(this.mprog);
        gl.uniformMatrix4fv(this.muView, false, view);
        gl.uniformMatrix4fv(this.muProj, false, proj);
        gl.uniform1i(this.muTex, 0);
        gl.uniform1i(this.muLit, 0);
        this._drawMesh(d);
      }
    }
    gl.bindVertexArray(null);
    gl.depthMask(true);
  }

  _drawMesh(d) {
    const gl = this.gl;
    const g = d.geom || this.cubeGeom;
    gl.bindVertexArray(g.vao);
    gl.bindTexture(gl.TEXTURE_2D, d.texture || this.white);
    gl.bindBuffer(gl.ARRAY_BUFFER, g.instBuf);
    gl.bufferData(gl.ARRAY_BUFFER, d.instances.subarray(0, d.count * MESH_FLOATS_PER_INSTANCE), gl.DYNAMIC_DRAW);
    gl.drawElementsInstanced(gl.TRIANGLES, g.indexCount, g.indexType, 0, d.count);
    gl.bindVertexArray(null);
  }
}

// unit cube fallback for meshes that fail to decode
function buildCubeMesh() {
  const faces = [
    [[0, 0, 1], [[-1, -1, 1], [1, -1, 1], [1, 1, 1], [-1, 1, 1]]],
    [[0, 0, -1], [[1, -1, -1], [-1, -1, -1], [-1, 1, -1], [1, 1, -1]]],
    [[1, 0, 0], [[1, -1, 1], [1, -1, -1], [1, 1, -1], [1, 1, 1]]],
    [[-1, 0, 0], [[-1, -1, -1], [-1, -1, 1], [-1, 1, 1], [-1, 1, -1]]],
    [[0, 1, 0], [[-1, 1, 1], [1, 1, 1], [1, 1, -1], [-1, 1, -1]]],
    [[0, -1, 0], [[-1, -1, -1], [1, -1, -1], [1, -1, 1], [-1, -1, 1]]],
  ];
  const positions = [], normals = [], uvs = [], indices = [];
  let vi = 0;
  for (const [n, c] of faces) {
    for (let k = 0; k < 4; k++) {
      positions.push(c[k][0] * 0.5, c[k][1] * 0.5, c[k][2] * 0.5);
      normals.push(n[0], n[1], n[2]);
      uvs.push(k === 1 || k === 2 ? 1 : 0, k >= 2 ? 1 : 0);
    }
    indices.push(vi, vi + 1, vi + 2, vi, vi + 2, vi + 3);
    vi += 4;
  }
  return {
    positions: new Float32Array(positions),
    normals: new Float32Array(normals),
    uvs: new Float32Array(uvs),
    indices: new Uint16Array(indices),
  };
}

export function makeTexture(gl, w, h, rgba) {
  const tex = gl.createTexture();
  gl.bindTexture(gl.TEXTURE_2D, tex);
  gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, w, h, 0, gl.RGBA, gl.UNSIGNED_BYTE, rgba);
  gl.generateMipmap(gl.TEXTURE_2D);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR_MIPMAP_LINEAR);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
  return tex;
}

function makeProgram(gl, vs, fs) {
  const p = gl.createProgram();
  for (const [type, src] of [[gl.VERTEX_SHADER, vs], [gl.FRAGMENT_SHADER, fs]]) {
    const sh = gl.createShader(type); gl.shaderSource(sh, src); gl.compileShader(sh);
    if (!gl.getShaderParameter(sh, gl.COMPILE_STATUS)) throw new Error('shader: ' + gl.getShaderInfoLog(sh));
    gl.attachShader(p, sh);
  }
  gl.linkProgram(p);
  if (!gl.getProgramParameter(p, gl.LINK_STATUS)) throw new Error('link: ' + gl.getProgramInfoLog(p));
  return p;
}

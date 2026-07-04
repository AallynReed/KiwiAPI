// WebGL2 instanced-billboard renderer for the particle simulation.
import { perspective, lookAt } from './glmath.js';

const VERT = `#version 300 es
precision highp float;
layout(location=0) in vec2 aCorner;   // -0.5..0.5 quad corner
layout(location=1) in vec2 aUV;       // 0..1
layout(location=2) in vec3 aCenter;
layout(location=3) in vec2 aSize;
layout(location=4) in vec4 aColor;
layout(location=5) in float aRot;
layout(location=6) in vec4 aUVRect;   // u0,v0,du,dv
uniform mat4 uView, uProj;
out vec2 vUV; out vec4 vColor;
void main(){
  float s = sin(aRot), c = cos(aRot);
  vec2 corner = vec2(aCorner.x*c - aCorner.y*s, aCorner.x*s + aCorner.y*c) * aSize;
  vec3 right = vec3(uView[0][0], uView[1][0], uView[2][0]);
  vec3 up    = vec3(uView[0][1], uView[1][1], uView[2][1]);
  vec3 world = aCenter + right*corner.x + up*corner.y;
  gl_Position = uProj * uView * vec4(world, 1.0);
  vUV = aUVRect.xy + aUV * aUVRect.zw;
  vColor = aColor;
}`;

const FRAG = `#version 300 es
precision highp float;
in vec2 vUV; in vec4 vColor;
uniform sampler2D uTex;
out vec4 frag;
void main(){
  vec4 t = texture(uTex, vUV);
  vec4 c = t * vColor;
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

// Mesh program: flat-shaded instanced boxes (proxy for .pkmm mesh particles).
const MVERT = `#version 300 es
precision highp float;
layout(location=0) in vec3 aPos;
layout(location=1) in vec3 aNormal;
layout(location=2) in vec3 aCenter;
layout(location=3) in vec3 aHalf;
layout(location=4) in vec4 aColor;
uniform mat4 uView, uProj;
out vec3 vN; out vec4 vColor;
void main(){
  vec3 world = aCenter + aPos * aHalf;
  gl_Position = uProj*uView*vec4(world,1.0);
  vN = aNormal; vColor = aColor;
}`;
const MFRAG = `#version 300 es
precision highp float;
in vec3 vN; in vec4 vColor; out vec4 frag;
void main(){
  float l = max(dot(normalize(vN), normalize(vec3(0.4,0.85,0.5))),0.0)*0.7 + 0.35;
  frag = vec4(vColor.rgb*l, vColor.a);
}`;
export const MESH_FLOATS_PER_INSTANCE = 10; // center3, half3, color4

function buildCubeGeom() {
  const faces = [
    [[0, 0, 1], [[-1, -1, 1], [1, -1, 1], [1, 1, 1], [-1, 1, 1]]],
    [[0, 0, -1], [[1, -1, -1], [-1, -1, -1], [-1, 1, -1], [1, 1, -1]]],
    [[1, 0, 0], [[1, -1, 1], [1, -1, -1], [1, 1, -1], [1, 1, 1]]],
    [[-1, 0, 0], [[-1, -1, -1], [-1, -1, 1], [-1, 1, 1], [-1, 1, -1]]],
    [[0, 1, 0], [[-1, 1, 1], [1, 1, 1], [1, 1, -1], [-1, 1, -1]]],
    [[0, -1, 0], [[-1, -1, -1], [1, -1, -1], [1, -1, 1], [-1, -1, 1]]],
  ];
  const out = [];
  for (const [n, c] of faces) for (const i of [0, 1, 2, 0, 2, 3]) out.push(c[i][0] * 0.5, c[i][1] * 0.5, c[i][2] * 0.5, n[0], n[1], n[2]);
  return new Float32Array(out);
}

const QUAD = new Float32Array([
  // corner.xy, uv.xy
  -0.5, -0.5, 0, 1,
   0.5, -0.5, 1, 1,
  -0.5,  0.5, 0, 0,
   0.5,  0.5, 1, 0,
]);
const FLOATS_PER_INSTANCE = 3 + 2 + 4 + 1 + 4; // center,size,color,rot,uvrect = 14

export class Renderer {
  constructor(canvas) {
    const gl = canvas.getContext('webgl2', { alpha: true, premultipliedAlpha: false, antialias: true });
    if (!gl) throw new Error('WebGL2 not available');
    this.gl = gl; this.canvas = canvas;
    this.prog = makeProgram(gl, VERT, FRAG);
    this.uView = gl.getUniformLocation(this.prog, 'uView');
    this.uProj = gl.getUniformLocation(this.prog, 'uProj');
    this.uTex = gl.getUniformLocation(this.prog, 'uTex');

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
    setup(2, 3, 0); setup(3, 2, 12); setup(4, 4, 20); setup(5, 1, 36); setup(6, 4, 40);
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

    // mesh (instanced box) program + VAO
    this.mprog = makeProgram(gl, MVERT, MFRAG);
    this.muView = gl.getUniformLocation(this.mprog, 'uView');
    this.muProj = gl.getUniformLocation(this.mprog, 'uProj');
    this.mvao = gl.createVertexArray();
    gl.bindVertexArray(this.mvao);
    this.cubeBuf = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, this.cubeBuf);
    gl.bufferData(gl.ARRAY_BUFFER, buildCubeGeom(), gl.STATIC_DRAW);
    gl.enableVertexAttribArray(0); gl.vertexAttribPointer(0, 3, gl.FLOAT, false, 24, 0);
    gl.enableVertexAttribArray(1); gl.vertexAttribPointer(1, 3, gl.FLOAT, false, 24, 12);
    this.minstBuf = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, this.minstBuf);
    const ms = MESH_FLOATS_PER_INSTANCE * 4;
    const msetup = (loc, size, off) => { gl.enableVertexAttribArray(loc); gl.vertexAttribPointer(loc, size, gl.FLOAT, false, ms, off); gl.vertexAttribDivisor(loc, 1); };
    msetup(2, 3, 0); msetup(3, 3, 12); msetup(4, 4, 24);
    gl.bindVertexArray(null);

    this.instData = new Float32Array(20000 * FLOATS_PER_INSTANCE);
    this.white = makeTexture(gl, 1, 1, new Uint8ClampedArray([255, 255, 255, 255]));
    this.cam = { az: 0.6, el: 0.3, dist: 14, target: [0, 1.5, 0] };
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

  draw(draws, ribbons = [], meshes = []) {
    // draws:   [{ texture, blend, instances: Float32Array, count, drawOrder }]
    // ribbons: [{ texture, blend, vertices: Float32Array, count, drawOrder }]  (count = vertices)
    // meshes:  [{ instances: Float32Array, count, drawOrder }]  (instanced boxes)
    const gl = this.gl; this.resize();
    gl.viewport(0, 0, this.canvas.width, this.canvas.height);
    gl.clearColor(0.05, 0.05, 0.07, 1); gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);
    gl.enable(gl.DEPTH_TEST); gl.depthMask(false); gl.enable(gl.BLEND);

    const aspect = this.canvas.width / this.canvas.height;
    const proj = perspective(60 * Math.PI / 180, aspect, 0.1, 1000);
    const eye = this.eyePosition();
    const view = lookAt(eye, this.cam.target, [0, 1, 0]);
    gl.activeTexture(gl.TEXTURE0);

    // opaque mesh proxies first (depth write on, so transparent passes sort against them)
    if (meshes.length) {
      gl.useProgram(this.mprog);
      gl.uniformMatrix4fv(this.muView, false, view);
      gl.uniformMatrix4fv(this.muProj, false, proj);
      gl.bindVertexArray(this.mvao);
      gl.depthMask(true); gl.disable(gl.BLEND);
      for (const d of meshes) {
        if (!d.count) continue;
        gl.bindBuffer(gl.ARRAY_BUFFER, this.minstBuf);
        gl.bufferData(gl.ARRAY_BUFFER, d.instances.subarray(0, d.count * MESH_FLOATS_PER_INSTANCE), gl.DYNAMIC_DRAW);
        gl.drawArraysInstanced(gl.TRIANGLES, 0, 36, d.count);
      }
      gl.enable(gl.BLEND); gl.depthMask(false);
      gl.bindVertexArray(null);
    }

    // ribbons next (usually behind the billboard glow)
    if (ribbons.length) {
      gl.useProgram(this.rprog);
      gl.uniformMatrix4fv(this.ruView, false, view);
      gl.uniformMatrix4fv(this.ruProj, false, proj);
      gl.uniform1i(this.ruTex, 0);
      gl.bindVertexArray(this.rvao);
      ribbons.sort((a, b) => a.drawOrder - b.drawOrder);
      for (const d of ribbons) {
        if (!d.count) continue;
        if (d.blend === 'add') gl.blendFunc(gl.SRC_ALPHA, gl.ONE);
        else gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);
        gl.bindTexture(gl.TEXTURE_2D, d.texture || this.white);
        gl.bindBuffer(gl.ARRAY_BUFFER, this.rbuf);
        gl.bufferData(gl.ARRAY_BUFFER, d.vertices.subarray(0, d.count * RIBBON_FLOATS_PER_VERT), gl.DYNAMIC_DRAW);
        gl.drawArrays(gl.TRIANGLES, 0, d.count);
      }
      gl.bindVertexArray(null);
    }

    gl.useProgram(this.prog);
    gl.uniformMatrix4fv(this.uView, false, view);
    gl.uniformMatrix4fv(this.uProj, false, proj);
    gl.uniform1i(this.uTex, 0);
    gl.bindVertexArray(this.vao);
    draws.sort((a, b) => a.drawOrder - b.drawOrder);
    for (const d of draws) {
      if (!d.count) continue;
      if (d.blend === 'add') gl.blendFunc(gl.SRC_ALPHA, gl.ONE);
      else gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);
      gl.bindTexture(gl.TEXTURE_2D, d.texture || this.white);
      gl.bindBuffer(gl.ARRAY_BUFFER, this.instBuf);
      gl.bufferData(gl.ARRAY_BUFFER, d.instances.subarray(0, d.count * FLOATS_PER_INSTANCE), gl.DYNAMIC_DRAW);
      gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, d.count);
    }
    gl.bindVertexArray(null);
  }
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

export { FLOATS_PER_INSTANCE };

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
uniform int uMode;  // 0 screen, 1 viewpos, 2 axis, 3 spheroid, 4 planar, 5 capsule
out vec2 vUV; out vec2 vUV2; out vec4 vColor; out float vBlend; out float vCursor;
void main(){
  float s = sin(aRot), c = cos(aRot);
  vec2 rot = vec2(aCorner.x*c - aCorner.y*s, aCorner.x*s + aCorner.y*c);
  vec3 world;
  if (uMode == 5) {
    /* CAxialBillboarderCapsule (FUN_1808a3650): an axial quad plus a pointed cap at
       each end. N = normalize(P - eye), S = normalize(cross(axis, N)) * Size.x,
       U = cross(N, S) (the axis flattened onto the screen, length Size.x).
       aCorner.x is the vertex role: 0 Top+S, 1 Bot+S, 2 Bot-S, 3 Top-S, 4 Top+U, 5 Bot-U,
       with Top/Bot = P +- 0.5*axis (axis already carries AxisScale). */
    vec3 N = normalize(aCenter - uEye);
    vec3 S = cross(aAxis, N);
    float sl = length(S);
    S = sl > 1e-4 ? S / sl : normalize(cross(N, vec3(uView[0][0], uView[1][0], uView[2][0])));
    S *= aSize.x;
    vec3 U = cross(N, S);
    vec3 top = aCenter + 0.5*aAxis, bot = aCenter - 0.5*aAxis;
    int r = int(aCorner.x + 0.5);
    world = r == 0 ? top + S : r == 1 ? bot + S : r == 2 ? bot - S : r == 3 ? top - S : r == 4 ? top + U : bot - U;
  } else if (uMode == 3) {
    /* Spheroidal, transcribed from CAxialBillboarderSpheroidal's position generator
       (FUN_1808a25a0 in HH-Bridge_r.dll). The engine computes, per particle:

           N = normalize(particlePos - cameraPos)
           S = normalize(cross(axis, N)) * Size.x
           U = 0.5*AxisScale*axis + cross(N, S)
           corners = particlePos +- U +- S

       The cross(N, S) term is what separates this from the plain axial quad, which
       has no such term. S is perpendicular to both the axis and the view, so it lies
       in the screen plane - and so does cross(N, S), with the same magnitude. When
       the axis swings toward the camera and the axis term stops contributing any
       screen extent, the quad still spans |S| both ways and settles into a round blob
       instead of collapsing to a sliver.

       Size.x is the HALF width here, as it is for screen quads; only the planar
       billboarder applies a 0.5. Size.y is not
       read at all, and the length comes purely from the axis vector, which already
       carries AxisScale (see packBillboards). */
    float L = length(aAxis);
    vec3 N = normalize(aCenter - uEye);
    vec3 dir = L > 1e-5 ? aAxis / L : vec3(uView[0][1], uView[1][1], uView[2][1]);
    vec3 S = cross(dir, N);
    float sl = length(S);
    // engine builds a fallback perpendicular when the axis is parallel to the view
    S = sl > 1e-5 ? S / sl : normalize(cross(N, vec3(uView[0][0], uView[1][0], uView[2][0])));
    S *= aSize.x;
    vec3 U = dir * (0.5*L) + cross(N, S);
    world = aCenter + U*(aCorner.y*2.0) + S*(aCorner.x*2.0);
  } else if (uMode == 2) {
    // CAxialBillboarderQuad: S = normalize(cross(axis, viewDir)) * Size.x, T = 0.5*AxisScale*axis,
    // corners = P +- S +- T. Same half-width-is-Size.x convention as the spheroidal above.
    float L = length(aAxis);
    vec3 toEye = normalize(uEye - aCenter);
    vec3 dir = L > 1e-5 ? aAxis / L : vec3(uView[0][1], uView[1][1], uView[2][1]);
    vec3 side = cross(dir, toEye);
    float sl = length(side);
    side = sl > 1e-5 ? side / sl : vec3(uView[0][0], uView[1][0], uView[2][0]);
    world = aCenter + side*(aCorner.x*2.0*aSize.x) + dir*(aCorner.y*L);
  } else if (uMode == 4) {
    /* CPlanarBillboarderQuad builds X = normalize(cross(Axis, Axis2)) and
       Y = cross(X, Axis2), then spends Size.x on X and Size.y on Y. So the width runs
       ACROSS the axis field and the height along it - the opposite of the obvious
       reading, and invisible while Size.x == Size.y. */
    vec3 n = length(aAxis2) > 1e-5 ? normalize(aAxis2) : vec3(0.0, 1.0, 0.0);
    vec3 x = cross(aAxis, n);
    vec3 X = length(x) > 1e-5 ? normalize(x)
           : normalize(abs(n.y) < 0.99 ? cross(n, vec3(0.0, 1.0, 0.0)) : vec3(1.0, 0.0, 0.0));
    vec3 Y = cross(X, n);
    // rotate the in-plane basis around the normal
    vec3 Tr = X*c + Y*s, Br = -X*s + Y*c;
    world = aCenter + Tr*(aCorner.x*aSize.x) + Br*(aCorner.y*aSize.y);
  } else if (uMode == 1) {
    vec3 fwd = normalize(uEye - aCenter);
    vec3 right = normalize(cross(vec3(0.0, 1.0, 0.0), fwd));
    if (length(cross(vec3(0.0,1.0,0.0), fwd)) < 1e-4) right = vec3(1.0, 0.0, 0.0);
    vec3 up = cross(fwd, right);
    world = aCenter + right*(rot.x*2.0*aSize.x) + up*(rot.y*2.0*aSize.y);
  } else {
    /* CScreenBillboarderQuad (FUN_18089d520): corners = P +- Size*(right +- up) on the
       unit camera axes, so Size is the HALF extent. aCorner is +-0.5, hence the 2. */
    vec3 right = vec3(uView[0][0], uView[1][0], uView[2][0]);
    vec3 up    = vec3(uView[0][1], uView[1][1], uView[2][1]);
    world = aCenter + right*(rot.x*2.0*aSize.x) + up*(rot.y*2.0*aSize.y);
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
uniform sampler2D uDepth;
uniform int uHasRemap;
uniform int uKind;   // 0 alpha, 1 additive, 2 alphablend_additive, 3 additive_noalpha, 4 alpha-weighted add
uniform float uSoft;     // SoftnessDistance in world units; 0 = not a _Soft material
uniform float uDissolve; // DissolveWidth from the renderer's UserData; 0 = plain alpha
uniform vec2 uInvRes;
uniform vec2 uClip;  // near, far
out vec4 frag;
float linearZ(float z){ float n = uClip.x, f = uClip.y; return (2.0*n*f) / (f + n - (z*2.0 - 1.0)*(f - n)); }
void main(){
  vec4 t = mix(texture(uTex, vUV), texture(uTex, vUV2), vBlend);
  // the remapper REPLACES the sampled alpha before anything else consumes it
  float texA = t.a;
  if (uHasRemap == 1) texA = texture(uRemap, vec2(clamp(t.a, 0.0, 1.0), clamp(vCursor, 0.0, 1.0))).r;

  /* Dissolve, transcribed from Trove's own particle shader. A renderer tagged
     UserData "dissolve <width>" stops treating the particle's alpha as opacity and
     uses it to sweep an erosion threshold through the texture instead, so the sprite
     burns away from its faintest pixels inward rather than fading uniformly. */
  vec4 c;
  if (uDissolve > 0.0) {
    float ta = (uKind == 3) ? sqrt(dot(t.rgb, vec3(0.299, 0.387, 0.314))) : texA;
    ta = clamp((ta - 0.1) / (0.2 - 0.1), 0.0, 1.0);
    float upper = (1.0 - vColor.a) * (1.0 + uDissolve);
    float lower = max(0.0, upper - uDissolve);
    // the engine divides by (upper-lower), which is 0 at full alpha; take the limit
    // rather than the NaN, which is what that case converges to anyway
    float strength = clamp((texA - lower) / max(upper - lower, 1e-5), 0.0, 1.0);
    c = vec4(t.rgb * vColor.rgb, ta * strength);
  } else {
    c = vec4(t.rgb * vColor.rgb, texA * vColor.a);
  }
  // Soft particles fade out as the quad nears the opaque surface behind it, so smoke
  // and fire sink into the ground instead of showing a hard intersection seam.
  float soft = 1.0;
  if (uSoft > 0.0) {
    float behind = linearZ(texture(uDepth, gl_FragCoord.xy * uInvRes).r);
    soft = clamp((behind - linearZ(gl_FragCoord.z)) / uSoft, 0.0, 1.0);
  }
  /* Blend maths transcribed from Trove's own particle pixel shader (embedded HLSL in
     Trove_x64.exe), whose base permutation is:
         diffuse = lerp(frameA, frameB, FrameLerp) * In.Color
         if (IsAlphaMultiply) diffuse *= diffuse.w      // premultiply
         if (IsAdditive)      diffuse.w  = 0            // add, via the same blend state
     Additive_NoAlpha is the IsAdditive && !IsAlphaMultiply case, so it carries NO
     alpha term at all, and the engine applies no alpha test on additive materials. */
  // Additive_NoAlpha zeroes alpha outright, so dissolve cannot reach the output here
  if (uKind == 3) { frag = vec4(c.rgb * soft, 1.0); return; }
  c.a *= soft;
  if (uKind == 2 || uKind == 4) { frag = vec4(c.rgb * soft, c.a); return; }
  if (uKind == 1) { frag = c; return; }
  if (c.a < 0.002) discard;   // AlphaTestMode GreaterOrEqual, AlphaTestValue 0.002
  frag = c;
}`;

/* Ground: one quad under the effect. It is what gives soft particles an opaque
   surface to fade against, and it stops fire and smoke floating in a void. Dark and
   vignetted so it reads as a stage rather than a slab, and it sits at the bottom of
   the effect's own footprint so it can never slice through a centred aura. */
const GVERT = `#version 300 es
precision highp float;
layout(location=0) in vec2 aCorner;
uniform mat4 uView, uProj;
uniform vec3 uCentre;
uniform float uSize;
out vec2 vXZ;
void main(){
  vXZ = aCorner * 2.0;
  gl_Position = uProj * uView * vec4(uCentre.x + aCorner.x*uSize, uCentre.y, uCentre.z + aCorner.y*uSize, 1.0);
}`;

const GFRAG = `#version 300 es
precision highp float;
in vec2 vXZ;
out vec4 frag;
void main(){
  float d = clamp(1.0 - length(vXZ), 0.0, 1.0);
  frag = vec4(mix(vec3(0.05,0.05,0.07), vec3(0.115,0.115,0.145), d*d), 1.0);
}`;

// Ribbon program: world-space triangles shaded by the billboard fragment shader, so
// ribbons get the same material kinds, soft fade and alpha remapper.
const RVERT = `#version 300 es
precision highp float;
layout(location=0) in vec3 aPos;
layout(location=1) in vec2 aUV;
layout(location=2) in vec4 aColor;
layout(location=3) in float aCursor;
layout(location=4) in vec2 aQuv;      // quad corner (0/1), CorrectDeformation only
layout(location=5) in vec4 aFac;      // UVFactors
layout(location=6) in vec4 aSO;       // UVScaleAndOffset
uniform mat4 uView, uProj;
out vec2 vUV; out vec2 vUV2; out vec4 vColor; out float vBlend; out float vCursor;
out vec2 vQuv; out vec4 vFac; out vec4 vSO;
void main(){ gl_Position = uProj*uView*vec4(aPos,1.0); vUV=aUV; vUV2=aUV; vColor=aColor; vBlend=0.0; vCursor=aCursor;
  vQuv=aQuv; vFac=aFac; vSO=aSO; }`;

/* Ribbon Quality = CorrectDeformation, from the Sprites_Ribbons.hbo graph: the corner
   UV is divided by the interpolated UVFactors per triangle of the quad (split along
   u+v = 1), which undoes the affine stretch across a trapezoid, then RotateUV swaps
   and UVScaleAndOffset maps it onto the texture. */
const RFRAG = FRAG
  .replace('in vec2 vUV; in vec2 vUV2;', 'in vec2 vUV; in vec2 vUV2; in vec2 vQuv; in vec4 vFac; in vec4 vSO;\nuniform int uCorrect; uniform int uRotate;')
  .replace('vec4 t = mix(texture(uTex, vUV), texture(uTex, vUV2), vBlend);', `vec2 ruv = vUV;
  if (uCorrect == 1) {
    float tri = clamp(1e10 * (vQuv.x + vQuv.y - 1.0), 0.0, 1.0);
    vec2 c = mix(vQuv / vFac.xy, vec2(1.0) - (vec2(1.0) - vQuv) / vFac.zw, tri);
    if (uRotate == 1) c = c.yx;
    ruv = c * vSO.xy + vSO.zw;
  }
  vec4 t = texture(uTex, ruv);`);

export const RIBBON_FLOATS_PER_VERT = 20; // pos3, uv2, color4, cursor1, quv2, fac4, so4

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
/* Trove's particle mesh pixel shader is unlit: texture * colour, nothing else. Solid
   draws opaque with depth write and no alpha test; the additive kinds use _blend(). */
const MFRAG = `#version 300 es
precision highp float;
in vec3 vN; in vec2 vUV; in vec4 vColor;
uniform sampler2D uTex;
uniform int uLit;      // 1 = Solid (opaque)
out vec4 frag;
void main(){
  vec4 c = texture(uTex, vUV) * vColor;
  frag = uLit == 1 ? vec4(c.rgb, 1.0) : c;
}`;
export const MESH_FLOATS_PER_INSTANCE = 16; // basis 9, center 3, color 4

// Capsule: 4 triangles over 6 roles (the engine's index list), role + diagonal UV per
// vertex: the +S edge maps to (1,1), -S to (0,0), the top tip (1,0), the bottom tip (0,1).
const CAPSULE_UV = [[1, 1], [1, 1], [0, 0], [0, 0], [1, 0], [0, 1]];
const CAPSULE = new Float32Array([0, 1, 2, 2, 3, 0, 3, 4, 0, 1, 5, 2].flatMap((r) => [r, 0, CAPSULE_UV[r][0], CAPSULE_UV[r][1]]));

const QUAD = new Float32Array([
  // corner.xy, uv.xy
  -0.5, -0.5, 0, 1,
   0.5, -0.5, 1, 1,
  -0.5,  0.5, 0, 0,
   0.5,  0.5, 1, 0,
]);

export class Renderer {
  constructor(canvas) {
    // Opaque like the game's back buffer: blending also writes destination alpha, and an
    // alpha canvas let every additive quad (alpha 1 edge to edge) show through as a square.
    const gl = canvas.getContext('webgl2', { alpha: false, antialias: true });
    if (!gl) throw new Error('WebGL2 not available');
    this.gl = gl; this.canvas = canvas;

    // billboard program
    this.prog = makeProgram(gl, VERT, FRAG);
    const u = (n) => gl.getUniformLocation(this.prog, n);
    this.u = { view: u('uView'), proj: u('uProj'), eye: u('uEye'), mode: u('uMode'), tex: u('uTex'), remap: u('uRemap'), hasRemap: u('uHasRemap'), kind: u('uKind'), depth: u('uDepth'), soft: u('uSoft'), dissolve: u('uDissolve'), invRes: u('uInvRes'), clip: u('uClip') };
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
    // capsule billboards: same instance data, their own 12-vertex shape
    this.cvao = gl.createVertexArray();
    gl.bindVertexArray(this.cvao);
    const capBuf = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, capBuf);
    gl.bufferData(gl.ARRAY_BUFFER, CAPSULE, gl.STATIC_DRAW);
    gl.enableVertexAttribArray(0); gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 16, 0);
    gl.enableVertexAttribArray(1); gl.vertexAttribPointer(1, 2, gl.FLOAT, false, 16, 8);
    gl.bindBuffer(gl.ARRAY_BUFFER, this.instBuf);
    setup(2, 3, 0); setup(3, 2, 12); setup(4, 4, 20); setup(5, 1, 36); setup(6, 4, 40);
    setup(7, 4, 56); setup(8, 1, 72); setup(9, 3, 76); setup(10, 3, 88); setup(11, 1, 100);
    gl.bindVertexArray(null);

    // ribbon program + its own VAO/buffer
    this.rprog = makeProgram(gl, RVERT, RFRAG);
    const ru = (n) => gl.getUniformLocation(this.rprog, n);
    this.ru = { view: ru('uView'), proj: ru('uProj'), tex: ru('uTex'), remap: ru('uRemap'), hasRemap: ru('uHasRemap'), kind: ru('uKind'), depth: ru('uDepth'), soft: ru('uSoft'), dissolve: ru('uDissolve'), invRes: ru('uInvRes'), clip: ru('uClip'), correct: ru('uCorrect'), rotate: ru('uRotate') };
    // TextureRepeat ribbons wrap instead of clamping
    this.repeatSampler = gl.createSampler();
    gl.samplerParameteri(this.repeatSampler, gl.TEXTURE_WRAP_S, gl.REPEAT);
    gl.samplerParameteri(this.repeatSampler, gl.TEXTURE_WRAP_T, gl.REPEAT);
    gl.samplerParameteri(this.repeatSampler, gl.TEXTURE_MIN_FILTER, gl.LINEAR_MIPMAP_LINEAR);
    gl.samplerParameteri(this.repeatSampler, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    this.rvao = gl.createVertexArray();
    gl.bindVertexArray(this.rvao);
    this.rbuf = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, this.rbuf);
    const rstride = RIBBON_FLOATS_PER_VERT * 4;
    gl.enableVertexAttribArray(0); gl.vertexAttribPointer(0, 3, gl.FLOAT, false, rstride, 0);
    gl.enableVertexAttribArray(1); gl.vertexAttribPointer(1, 2, gl.FLOAT, false, rstride, 12);
    gl.enableVertexAttribArray(2); gl.vertexAttribPointer(2, 4, gl.FLOAT, false, rstride, 20);
    gl.enableVertexAttribArray(3); gl.vertexAttribPointer(3, 1, gl.FLOAT, false, rstride, 36);
    gl.enableVertexAttribArray(4); gl.vertexAttribPointer(4, 2, gl.FLOAT, false, rstride, 40);
    gl.enableVertexAttribArray(5); gl.vertexAttribPointer(5, 4, gl.FLOAT, false, rstride, 48);
    gl.enableVertexAttribArray(6); gl.vertexAttribPointer(6, 4, gl.FLOAT, false, rstride, 64);
    gl.bindVertexArray(null);

    // mesh program (geometries are created per mesh via makeMeshGeometry)
    this.mprog = makeProgram(gl, MVERT, MFRAG);
    this.muView = gl.getUniformLocation(this.mprog, 'uView');
    this.muProj = gl.getUniformLocation(this.mprog, 'uProj');
    this.muTex = gl.getUniformLocation(this.mprog, 'uTex');
    this.muLit = gl.getUniformLocation(this.mprog, 'uLit');
    this.cubeGeom = this.makeMeshGeometry(buildCubeMesh());

    // ground program (reuses the billboard quad buffer, corner attribute only)
    this.gprog = makeProgram(gl, GVERT, GFRAG);
    this.gu = {
      view: gl.getUniformLocation(this.gprog, 'uView'), proj: gl.getUniformLocation(this.gprog, 'uProj'),
      centre: gl.getUniformLocation(this.gprog, 'uCentre'), size: gl.getUniformLocation(this.gprog, 'uSize'),
    };
    this.gvao = gl.createVertexArray();
    gl.bindVertexArray(this.gvao);
    gl.bindBuffer(gl.ARRAY_BUFFER, this.quadBuf);
    gl.enableVertexAttribArray(0); gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 16, 0);
    gl.bindVertexArray(null);

    // Opaque pass target. Particles cannot sample the depth buffer they are drawing
    // into, so the ground and solid meshes render here first; the result is blitted
    // to the screen and the depth texture is handed to the particle shader.
    this.fbo = gl.createFramebuffer();
    this.colorTex = gl.createTexture();
    this.depthTex = gl.createTexture();
    this.fboW = 0; this.fboH = 0;

    this.white = makeTexture(gl, 1, 1, new Uint8ClampedArray([255, 255, 255, 255]));
    this.cam = { az: 0.6, el: 0.3, dist: 14, target: [0, 1.5, 0] };
    // set by the viewer once it has measured the effect; null = no ground
    this.ground = null;
  }

  _ensureTargets(w, h) {
    const gl = this.gl;
    if (this.fboW === w && this.fboH === h) return;
    gl.bindTexture(gl.TEXTURE_2D, this.colorTex);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA8, w, h, 0, gl.RGBA, gl.UNSIGNED_BYTE, null);
    for (const [k, v] of [[gl.TEXTURE_MIN_FILTER, gl.NEAREST], [gl.TEXTURE_MAG_FILTER, gl.NEAREST],
      [gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE], [gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE]]) gl.texParameteri(gl.TEXTURE_2D, k, v);
    gl.bindTexture(gl.TEXTURE_2D, this.depthTex);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.DEPTH_COMPONENT24, w, h, 0, gl.DEPTH_COMPONENT, gl.UNSIGNED_INT, null);
    for (const [k, v] of [[gl.TEXTURE_MIN_FILTER, gl.NEAREST], [gl.TEXTURE_MAG_FILTER, gl.NEAREST],
      [gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE], [gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE]]) gl.texParameteri(gl.TEXTURE_2D, k, v);
    gl.bindFramebuffer(gl.FRAMEBUFFER, this.fbo);
    gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, this.colorTex, 0);
    gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.DEPTH_ATTACHMENT, gl.TEXTURE_2D, this.depthTex, 0);
    gl.bindFramebuffer(gl.FRAMEBUFFER, null);
    gl.bindTexture(gl.TEXTURE_2D, null);
    this.fboW = w; this.fboH = h;
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
    else if (kind === 4) gl.blendFunc(gl.SRC_ALPHA, gl.ONE);
    else if (kind === 3) gl.blendFunc(gl.ONE, gl.ONE);
    else gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);
  }

  // items: mixed draw list, each { type: 'billboard'|'ribbon'|'mesh', drawOrder, ... }
  //   billboard: { texture, remapTexture?, kind, mode, instances, count }
  //   ribbon:    { texture, remapTexture?, kind, soft, repeat, correct, rotate, vertices, count }   (count = vertices)
  //   mesh:      { geom, texture, lit, kind, instances, count }
  draw(items) {
    const gl = this.gl; this.resize();
    const W = this.canvas.width, H = this.canvas.height;
    this._ensureTargets(W, H);
    const NEAR = 0.1, FAR = 1000;

    const aspect = W / H;
    const proj = perspective(60 * Math.PI / 180, aspect, NEAR, FAR);
    const eye = this.eyePosition();
    const view = lookAt(eye, this.cam.target, [0, 1, 0]);

    // opaque (lit solid) meshes first with depth write, then everything else sorted
    const solid = items.filter((d) => d.type === 'mesh' && d.lit);
    /* Batch order, as CRenderList::Render (FUN_1803427d0) keys it: DrawOrder first; within
       one DrawOrder, materials with SortMode BackToFront (every alpha-blended kind) add
       -distance^2 from the camera to the batch position in the low bits, so additive
       batches (no sort, low bits 0) go first and blended ones follow farthest first. */
    const d2 = (d) => d.center ? (d.center[0] - eye[0]) ** 2 + (d.center[1] - eye[1]) ** 2 + (d.center[2] - eye[2]) ** 2 : 0;
    const sorted = (d) => d.kind === 0 || d.kind === 2 || d.kind === 4;
    const trans = items.filter((d) => !(d.type === 'mesh' && d.lit)).sort((a, b) =>
      ((a.drawOrder || 0) - (b.drawOrder || 0)) ||
      ((sorted(a) ? 1 : 0) - (sorted(b) ? 1 : 0)) ||
      (sorted(a) ? d2(b) - d2(a) : 0));

    /* The opaque geometry is drawn twice: once into the offscreen target purely to
       fill a depth texture the particle shader can sample, then again on screen for
       the visible pixels. That is cheaper and far more portable than blitting depth
       between framebuffers, which needs the two depth formats to agree - and it is
       one quad plus the rare solid mesh. */
    const drawOpaque = () => {
      gl.enable(gl.DEPTH_TEST); gl.depthMask(true); gl.disable(gl.BLEND);
      gl.activeTexture(gl.TEXTURE0);
      if (this.ground) {
        gl.useProgram(this.gprog);
        gl.uniformMatrix4fv(this.gu.view, false, view);
        gl.uniformMatrix4fv(this.gu.proj, false, proj);
        gl.uniform3f(this.gu.centre, this.ground.centre[0], this.ground.y, this.ground.centre[2]);
        gl.uniform1f(this.gu.size, this.ground.size);
        gl.bindVertexArray(this.gvao);
        gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
        gl.bindVertexArray(null);
      }
      if (solid.length) {
        gl.useProgram(this.mprog);
        gl.uniformMatrix4fv(this.muView, false, view);
        gl.uniformMatrix4fv(this.muProj, false, proj);
        gl.uniform1i(this.muTex, 0);
        gl.uniform1i(this.muLit, 1);
        for (const d of solid) this._drawMesh(d);
      }
    };

    /* The depth pass runs every frame even with nothing opaque in it: an uncleared
       depth texture reads as ZERO, which is the near plane, and every soft particle
       would fade to nothing. Clearing to 1.0 makes "no ground" mean "infinitely far"
       and the fade a no-op, which is the behaviour we want. */
    const opaque = this.ground || solid.length;
    gl.bindFramebuffer(gl.FRAMEBUFFER, this.fbo);
    gl.viewport(0, 0, W, H);
    gl.clearColor(0.05, 0.05, 0.07, 1); gl.clearDepth(1);
    gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);
    if (opaque) drawOpaque();
    gl.bindFramebuffer(gl.FRAMEBUFFER, null);

    gl.viewport(0, 0, W, H);
    gl.clearColor(0.05, 0.05, 0.07, 1); gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);
    if (opaque) drawOpaque();
    gl.activeTexture(gl.TEXTURE2);
    gl.bindTexture(gl.TEXTURE_2D, this.depthTex);
    gl.activeTexture(gl.TEXTURE0);

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
          gl.uniform1i(this.u.depth, 2);
          gl.uniform2f(this.u.invRes, 1 / W, 1 / H);
          gl.uniform2f(this.u.clip, NEAR, FAR);
        }
        gl.bindVertexArray(d.mode === 5 ? this.cvao : this.vao);
        gl.uniform1i(this.u.mode, d.mode || 0);
        gl.uniform1i(this.u.kind, d.kind || 0);
        gl.uniform1f(this.u.soft, d.soft || 0);
        gl.uniform1f(this.u.dissolve, d.dissolve || 0);
        gl.uniform1i(this.u.hasRemap, d.remapTexture ? 1 : 0);
        gl.activeTexture(gl.TEXTURE1);
        gl.bindTexture(gl.TEXTURE_2D, d.remapTexture || this.white);
        gl.activeTexture(gl.TEXTURE0);
        gl.bindTexture(gl.TEXTURE_2D, d.texture || this.white);
        gl.bindBuffer(gl.ARRAY_BUFFER, this.instBuf);
        gl.bufferData(gl.ARRAY_BUFFER, d.instances.subarray(0, d.count * FLOATS_PER_INSTANCE), gl.DYNAMIC_DRAW);
        if (d.mode === 5) gl.drawArraysInstanced(gl.TRIANGLES, 0, 12, d.count);
        else gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, d.count);
      } else if (d.type === 'ribbon') {
        const u = this.ru;
        if (prog !== 'r') { prog = 'r';
          gl.useProgram(this.rprog);
          gl.uniformMatrix4fv(u.view, false, view);
          gl.uniformMatrix4fv(u.proj, false, proj);
          gl.uniform1i(u.tex, 0);
          gl.uniform1i(u.remap, 1);
          gl.uniform1i(u.depth, 2);
          gl.uniform2f(u.invRes, 1 / W, 1 / H);
          gl.uniform2f(u.clip, NEAR, FAR);
          gl.uniform1f(u.dissolve, 0);
          gl.bindVertexArray(this.rvao);
        }
        gl.uniform1i(u.kind, d.kind || 0);
        gl.uniform1f(u.soft, d.soft || 0);
        gl.uniform1i(u.hasRemap, d.remapTexture ? 1 : 0);
        gl.uniform1i(u.correct, d.correct ? 1 : 0);
        gl.uniform1i(u.rotate, d.rotate ? 1 : 0);
        gl.activeTexture(gl.TEXTURE1);
        gl.bindTexture(gl.TEXTURE_2D, d.remapTexture || this.white);
        gl.activeTexture(gl.TEXTURE0);
        gl.bindTexture(gl.TEXTURE_2D, d.texture || this.white);
        gl.bindSampler(0, d.repeat ? this.repeatSampler : null);
        gl.bindBuffer(gl.ARRAY_BUFFER, this.rbuf);
        gl.bufferData(gl.ARRAY_BUFFER, d.vertices.subarray(0, d.count * RIBBON_FLOATS_PER_VERT), gl.DYNAMIC_DRAW);
        gl.drawArrays(gl.TRIANGLES, 0, d.count);
        gl.bindSampler(0, null);
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

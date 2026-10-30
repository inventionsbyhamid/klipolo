import { FORMAT_NV12, type FrameMeta } from './frameTrailer';

export interface FrameDrawer {
  kind: 'webgl2' | 'canvas2d';
  draw(buf: ArrayBuffer, meta: FrameMeta): void;
  dispose(): void;
}

const VERTEX = `#version 300 es
in vec2 p; out vec2 uv;
void main(){ uv = vec2((p.x+1.)*.5, 1.-(p.y+1.)*.5); gl_Position = vec4(p,0.,1.); }`;

const RGBA = `#version 300 es
precision mediump float; in vec2 uv; uniform sampler2D t; out vec4 o;
void main(){ o = texture(t, uv); }`;

// BT.709 limited range back to RGB: the inverse of the engine's RGBA → NV12 converter (rgba_to_nv12.wgsl).
const NV12 = `#version 300 es
precision highp float; in vec2 uv; uniform sampler2D ty; uniform sampler2D tuv; out vec4 o;
void main(){
  float y = (texture(ty, uv).r * 255. - 16.) / 219.;
  vec2 c = (texture(tuv, uv).rg * 255. - 128.) / 224.;
  o = vec4(clamp(vec3(y + 1.5748 * c.y, y - .1873 * c.x - .4681 * c.y, y + 1.8556 * c.x), 0., 1.), 1.);
}`;

interface Plane {
  tex: WebGLTexture | null;
  w: number;
  h: number;
}

export function sizeCanvas(canvas: HTMLCanvasElement, meta: FrameMeta) {
  if (canvas.width !== meta.width || canvas.height !== meta.height) {
    canvas.width = meta.width;
    canvas.height = meta.height;
  }
}

/**
 * WebGL2 frames: RGBA stills and NV12 playback frames (an R8 luma and an RG8 chroma texture, converted in the
 * shader). Textures are immutable (`texStorage2D`), made again only when the size changes, and filled with
 * `texSubImage2D` reading the padded rows in place. Null when the canvas has no WebGL2.
 */
export function createWebGL2(canvas: HTMLCanvasElement): FrameDrawer | null {
  const gl = canvas.getContext('webgl2', {
    antialias: false,
    alpha: false,
    desynchronized: true,
    preserveDrawingBuffer: false,
  });
  if (!gl) return null;
  const compile = (type: number, src: string) => {
    const s = gl.createShader(type)!;
    gl.shaderSource(s, src);
    gl.compileShader(s);
    if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(s) ?? 'shader');
    return s;
  };
  const vertex = compile(gl.VERTEX_SHADER, VERTEX);
  const link = (fragment: string, samplers: string[]) => {
    const prog = gl.createProgram()!;
    gl.attachShader(prog, vertex);
    gl.attachShader(prog, compile(gl.FRAGMENT_SHADER, fragment));
    gl.bindAttribLocation(prog, 0, 'p');
    gl.linkProgram(prog);
    if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(prog) ?? 'link');
    gl.useProgram(prog);
    samplers.forEach((name, unit) => gl.uniform1i(gl.getUniformLocation(prog, name), unit));
    return prog;
  };
  const rgbaProg = link(RGBA, ['t']);
  const nv12Prog = link(NV12, ['ty', 'tuv']);
  gl.bindBuffer(gl.ARRAY_BUFFER, gl.createBuffer());
  gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 1, -1, -1, 1, 1, 1]), gl.STATIC_DRAW);
  gl.enableVertexAttribArray(0);
  gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);

  const planes: Record<'rgba' | 'y' | 'uv', Plane> = {
    rgba: { tex: null, w: 0, h: 0 },
    y: { tex: null, w: 0, h: 0 },
    uv: { tex: null, w: 0, h: 0 },
  };
  /** Binds `plane` on `unit`, making its immutable storage again if the size changed. */
  const bind = (plane: Plane, unit: number, internal: number, w: number, h: number) => {
    gl.activeTexture(gl.TEXTURE0 + unit);
    if (plane.tex && plane.w === w && plane.h === h) {
      gl.bindTexture(gl.TEXTURE_2D, plane.tex);
      return;
    }
    if (plane.tex) gl.deleteTexture(plane.tex);
    plane.tex = gl.createTexture();
    plane.w = w;
    plane.h = h;
    gl.bindTexture(gl.TEXTURE_2D, plane.tex);
    gl.texStorage2D(gl.TEXTURE_2D, 1, internal, w, h);
    for (const [k, v] of [
      [gl.TEXTURE_MIN_FILTER, gl.LINEAR],
      [gl.TEXTURE_MAG_FILTER, gl.LINEAR],
      [gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE],
      [gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE],
    ])
      gl.texParameteri(gl.TEXTURE_2D, k, v);
  };

  return {
    kind: 'webgl2',
    draw(buf, meta) {
      sizeCanvas(canvas, meta);
      gl.viewport(0, 0, meta.width, meta.height);
      const { width: w, height: h, stride } = meta;
      if (meta.format === FORMAT_NV12) {
        const cw = Math.ceil(w / 2);
        const ch = Math.ceil(h / 2);
        gl.useProgram(nv12Prog);
        bind(planes.y, 0, gl.R8, w, h);
        gl.pixelStorei(gl.UNPACK_ROW_LENGTH, stride);
        gl.texSubImage2D(
          gl.TEXTURE_2D,
          0,
          0,
          0,
          w,
          h,
          gl.RED,
          gl.UNSIGNED_BYTE,
          new Uint8Array(buf, 0, stride * h),
        );
        bind(planes.uv, 1, gl.RG8, cw, ch);
        gl.pixelStorei(gl.UNPACK_ROW_LENGTH, stride / 2);
        gl.texSubImage2D(
          gl.TEXTURE_2D,
          0,
          0,
          0,
          cw,
          ch,
          gl.RG,
          gl.UNSIGNED_BYTE,
          new Uint8Array(buf, stride * h, stride * ch),
        );
      } else {
        gl.useProgram(rgbaProg);
        bind(planes.rgba, 0, gl.RGBA8, w, h);
        // Rows are padded to 256 bytes (wgpu readback): read them in place, no repacking.
        gl.pixelStorei(gl.UNPACK_ROW_LENGTH, stride / 4);
        gl.texSubImage2D(
          gl.TEXTURE_2D,
          0,
          0,
          0,
          w,
          h,
          gl.RGBA,
          gl.UNSIGNED_BYTE,
          new Uint8Array(buf, 0, stride * h),
        );
      }
      gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
    },
    dispose() {
      gl.getExtension('WEBGL_lose_context')?.loseContext();
    },
  };
}

/** BT.709 limited-range NV12 → RGBA on the CPU: only the Canvas2D fallback (WebGL lost twice) ever needs it. */
export function nv12ToRgba(buf: ArrayBuffer, meta: FrameMeta, out: Uint8ClampedArray) {
  const { width: w, height: h, stride } = meta;
  const src = new Uint8Array(buf);
  const uvAt = stride * h;
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      const l = (src[y * stride + x] - 16) / 219;
      const c = uvAt + (y >> 1) * stride + (x & ~1);
      const u = (src[c] - 128) / 224;
      const v = (src[c + 1] - 128) / 224;
      const o = (y * w + x) * 4;
      out[o] = (l + 1.5748 * v) * 255;
      out[o + 1] = (l - 0.1873 * u - 0.4681 * v) * 255;
      out[o + 2] = (l + 1.8556 * u) * 255;
      out[o + 3] = 255;
    }
  }
}

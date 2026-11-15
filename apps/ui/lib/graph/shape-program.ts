/*
 * The node program for shapes (plan 198; the UI expert's design note, section 1): sigma's node-border program
 * with the disc test replaced by a signed distance to the node's shape, read from its `shape` attribute
 * (lib/graph/shapes.ts). One triangle per node, as before; the status ring, the selection ring and the agent ring are a
 * band of the same distance, so they follow every shape. Picking stays sigma's disc test.
 *
 * Adapted from @sigma/node-border 3.0.0 (createNodeBorderProgram with a border of attribute size and colour, then the
 * fill), under its MIT licence:
 *
 * Copyright (C) 2013-2024, Alexis Jacomy, Guillaume Plique, Benoît Simard https://www.sigmajs.org
 *
 * Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated
 * documentation files (the "Software"), to deal in the Software without restriction, including without limitation the
 * rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to
 * permit persons to whom the Software is furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in all copies or substantial portions of the
 * Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE
 * WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR
 * COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR
 * OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
 */
import type { Attributes } from "graphology-types";
import { NodeProgram, type ProgramInfo } from "sigma/rendering";
import type { NodeDisplayData, RenderParams } from "sigma/types";
import { floatColor } from "sigma/utils";
import { GEOMETRY, SHAPE_REACH, SHAPES } from "@/lib/graph/shapes";

// WebGL constants by value: the module is imported where there is no WebGL (tests, the server render).
const FLOAT = 5126;
const UNSIGNED_BYTE = 5121;
const TRIANGLES = 4;

const f = (v: number) => (Number.isInteger(v) ? `${v}.0` : String(v));

/** The GLSL body of `float shapeDistance(vec2 p, float shape)`: one branch per shape, from the same table as the 2D
 * paths, each a distance in units of the node's radius (negative inside). */
function shapeDistanceGlsl(): string {
  const branches = SHAPES.map((shape, i) => {
    const g = GEOMETRY[shape];
    let body: string;
    if (g.kind === "circle") body = "return length(p) - 1.0;";
    else if (g.kind === "ellipse") {
      const [rx, ry] = [f(g.rx), f(g.ry)];
      body = `float k0 = length(p / vec2(${rx}, ${ry})); float k1 = length(p / vec2(${f(g.rx * g.rx)}, ${f(g.ry * g.ry)})); if (k1 < 1e-6) return -${f(Math.min(g.rx, g.ry))}; return k0 * (k0 - 1.0) / k1;`;
    } else {
      const steps = g.points.map((a, k) => {
        const b = g.points[(k + 1) % g.points.length];
        return `m = min(m, side(p, vec2(${f(a[0])}, ${f(a[1])}), vec2(${f(b[0])}, ${f(b[1])}), inside));`;
      });
      // A segment (two points) has no inside.
      body = `float m = 1e9; float inside = ${g.points.length > 2 ? "1.0" : "0.0"}; ${steps.join(" ")} return (inside > 0.5 ? -m : m) - ${f(g.round)};`;
    }
    return `  ${i === 0 ? "" : "else "}if (shape < ${f(i + 0.5)}) { ${body} }`;
  });
  return `${branches.join("\n")}\n  return length(p) - 1.0;`;
}

const FRAGMENT = /* glsl */ `
precision highp float;

varying vec2 v_diffVector;
varying float v_radius;
varying vec4 v_color;

#ifndef PICKING_MODE
varying vec4 v_borderColor;
varying float v_borderSize;
varying float v_shape;
#endif

uniform float u_correctionRatio;

const float bias = 255.0 / 254.0;
const vec4 transparent = vec4(0.0, 0.0, 0.0, 0.0);

#ifndef PICKING_MODE
// The distance from p to segment ab; clears inside when p is right of ab (the points run counter-clockwise).
float side(vec2 p, vec2 a, vec2 b, inout float inside) {
  vec2 pa = p - a;
  vec2 ba = b - a;
  float h = clamp(dot(pa, ba) / dot(ba, ba), 0.0, 1.0);
  if (ba.x * pa.y - ba.y * pa.x < 0.0) inside = 0.0;
  return length(pa - ba * h);
}

float shapeDistance(vec2 p, float shape) {
${shapeDistanceGlsl()}
}
#endif

void main(void) {
  #ifdef PICKING_MODE
  if (length(v_diffVector) > v_radius)
    gl_FragColor = transparent;
  else {
    gl_FragColor = v_color;
    gl_FragColor.a *= bias;
  }
  #else
  float aa = 2.0 * u_correctionRatio;
  float d = shapeDistance(v_diffVector / v_radius, v_shape) * v_radius;
  float border = v_borderSize * v_radius;
  vec4 ring = v_borderColor;
  ring.a *= bias;
  vec4 fill = v_color;
  fill.a *= bias;
  // As node-border: a ring under a pixel is not drawn, and a fill under a pixel takes the ring's colour.
  if (border <= u_correctionRatio) ring = transparent;
  if (v_radius - border <= u_correctionRatio) fill = ring;
  if (d > 0.0) gl_FragColor = transparent;
  else if (d > -aa) gl_FragColor = mix(ring, transparent, (d + aa) / aa);
  else if (d > -border) gl_FragColor = ring;
  else if (d > -border - aa) gl_FragColor = mix(fill, ring, (d + border + aa) / aa);
  else gl_FragColor = fill;
  #endif
}
`;

const VERTEX = /* glsl */ `
attribute vec2 a_position;
attribute float a_size;
attribute float a_angle;

uniform mat3 u_matrix;
uniform float u_sizeRatio;
uniform float u_correctionRatio;

varying vec2 v_diffVector;
varying float v_radius;
varying vec4 v_color;

#ifdef PICKING_MODE
attribute vec4 a_id;
#else
attribute vec4 a_color;
attribute vec4 a_borderColor;
attribute float a_borderSize;
attribute float a_shape;
varying vec4 v_borderColor;
varying float v_borderSize;
varying float v_shape;
#endif

void main() {
  float size = a_size * u_correctionRatio / u_sizeRatio * 4.0;
  // The triangle's inner circle is the node's circle times SHAPE_REACH, so it holds every shape's farthest point.
  vec2 diffVector = size * ${f(SHAPE_REACH)} * vec2(cos(a_angle), sin(a_angle));
  vec2 position = a_position + diffVector;
  gl_Position = vec4((u_matrix * vec3(position, 1)).xy, 0, 1);
  v_radius = size / 2.0;
  v_diffVector = diffVector;
  #ifdef PICKING_MODE
  v_color = a_id;
  #else
  v_color = a_color;
  v_borderColor = a_borderColor;
  v_borderSize = a_borderSize;
  v_shape = a_shape;
  #endif
}
`;

const UNIFORMS = ["u_sizeRatio", "u_correctionRatio", "u_matrix"] as const;

/** The ring's share of the radius when a node has none set (node-border's default here before). */
const BORDER_SIZE = 0.15;

export class ShapeProgram<N extends Attributes = Attributes, E extends Attributes = Attributes, G extends Attributes = Attributes> extends NodeProgram<(typeof UNIFORMS)[number], N, E, G> {
  static readonly ANGLE_1 = 0;
  static readonly ANGLE_2 = (2 * Math.PI) / 3;
  static readonly ANGLE_3 = (4 * Math.PI) / 3;

  getDefinition() {
    return {
      VERTICES: 3,
      VERTEX_SHADER_SOURCE: VERTEX,
      FRAGMENT_SHADER_SOURCE: FRAGMENT,
      METHOD: TRIANGLES,
      UNIFORMS,
      ATTRIBUTES: [
        { name: "a_position", size: 2, type: FLOAT },
        { name: "a_id", size: 4, type: UNSIGNED_BYTE, normalized: true },
        { name: "a_size", size: 1, type: FLOAT },
        { name: "a_color", size: 4, type: UNSIGNED_BYTE, normalized: true },
        { name: "a_borderColor", size: 4, type: UNSIGNED_BYTE, normalized: true },
        { name: "a_borderSize", size: 1, type: FLOAT },
        { name: "a_shape", size: 1, type: FLOAT },
      ],
      CONSTANT_ATTRIBUTES: [{ name: "a_angle", size: 1, type: FLOAT }],
      CONSTANT_DATA: [[ShapeProgram.ANGLE_1], [ShapeProgram.ANGLE_2], [ShapeProgram.ANGLE_3]],
    };
  }

  processVisibleItem(nodeIndex: number, startIndex: number, data: NodeDisplayData) {
    const array = this.array;
    const d = data as NodeDisplayData & { borderColor?: string; borderSize?: number; shape?: number };
    array[startIndex++] = d.x;
    array[startIndex++] = d.y;
    array[startIndex++] = nodeIndex;
    array[startIndex++] = d.size;
    array[startIndex++] = floatColor(d.color);
    array[startIndex++] = floatColor(d.borderColor || "#000000");
    array[startIndex++] = d.borderSize || BORDER_SIZE;
    array[startIndex++] = d.shape ?? 0;
  }

  setUniforms(params: RenderParams, { gl, uniformLocations }: ProgramInfo) {
    gl.uniform1f(uniformLocations.u_correctionRatio, params.correctionRatio);
    gl.uniform1f(uniformLocations.u_sizeRatio, params.sizeRatio);
    gl.uniformMatrix3fv(uniformLocations.u_matrix, false, params.matrix);
  }
}

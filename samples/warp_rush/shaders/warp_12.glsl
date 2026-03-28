#version 100
precision mediump float;

varying lowp vec2 uv;
varying lowp vec4 color;

uniform sampler2D Texture;
uniform float u_time;
uniform vec2 u_resolution;
uniform float u_intensity;

vec3 palette(float t) {
    vec3 a = vec3(0.497, 0.672, 0.303);
    vec3 b = vec3(0.301, 0.698, 0.364);
    vec3 c = vec3(0.317, 0.660, 0.323);
    vec3 d = vec3(0.550, 0.130, 0.500);
    return a + b * cos(6.28318 * (c * t + d));
}

void main() {
    vec2 p = (uv - 0.5) * 2.0;
    p.x *= u_resolution.x / max(u_resolution.y, 1.0);

    float t = u_time * 0.870;
    float intensity = 0.2 + u_intensity * 1.35;

    float warpA = sin(p.y * 5.150 + t * 1.440) * 0.080;
    float warpB = cos(p.x * 2.000 - t * 0.900) * 0.116;
    vec2 q = p;
    q += vec2(warpA + warpB, sin((p.x + p.y) * 3.240 + t * 2.550) * 0.040) * intensity;

    float v = 0.0;
    v += sin(length(q) * 11.050 - t * 1.840);
    v += sin((q.x * 2.060 + q.y * 1.100) * 4.210 + t * 1.720);
    v += cos((q.x - q.y) * 2.560 - t * 1.620);
    v = 0.5 + 0.5 * (v / 3.0);

    float flicker = 0.5 + 0.5 * sin(t * 2.7 + v * 12.0 + q.x * 3.0);
    vec3 fx = palette(v + 0.12 * flicker);

    float vignette = pow(1.0 - smoothstep(0.2, 1.4, length(p)), 1.170);
    fx *= 0.35 + 0.65 * vignette;

    vec4 tex = texture2D(Texture, uv);
    gl_FragColor = vec4(fx * color.rgb, tex.a * color.a);
}

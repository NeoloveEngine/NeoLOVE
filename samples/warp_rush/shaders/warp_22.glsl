#version 100
precision mediump float;

varying lowp vec2 uv;
varying lowp vec4 color;

uniform sampler2D Texture;
uniform float u_time;
uniform vec2 u_resolution;
uniform float u_intensity;

vec3 palette(float t) {
    vec3 a = vec3(0.307, 0.675, 0.491);
    vec3 b = vec3(0.340, 0.263, 0.687);
    vec3 c = vec3(0.392, 0.215, 0.618);
    vec3 d = vec3(0.110, 0.490, 0.500);
    return a + b * cos(6.28318 * (c * t + d));
}

void main() {
    vec2 p = (uv - 0.5) * 2.0;
    p.x *= u_resolution.x / max(u_resolution.y, 1.0);

    float t = u_time * 0.870;
    float intensity = 0.2 + u_intensity * 1.35;

    float warpA = sin(p.y * 2.950 + t * 1.440) * 0.150;
    float warpB = cos(p.x * 4.520 - t * 2.140) * 0.116;
    vec2 q = p;
    q += vec2(warpA + warpB, sin((p.x + p.y) * 4.060 + t * 1.390) * 0.128) * intensity;

    float v = 0.0;
    v += sin(length(q) * 6.850 - t * 2.360);
    v += sin((q.x * 2.060 + q.y * 2.120) * 4.680 + t * 1.960);
    v += cos((q.x - q.y) * 2.560 - t * 1.620);
    v = 0.5 + 0.5 * (v / 3.0);

    float flicker = 0.5 + 0.5 * sin(t * 2.7 + v * 12.0 + q.x * 3.0);
    vec3 fx = palette(v + 0.12 * flicker);

    float vignette = pow(1.0 - smoothstep(0.2, 1.4, length(p)), 1.170);
    fx *= 0.35 + 0.65 * vignette;

    vec4 tex = texture2D(Texture, uv);
    gl_FragColor = vec4(fx * color.rgb, tex.a * color.a);
}

#version 100
precision mediump float;

varying lowp vec2 uv;
varying lowp vec4 color;

uniform sampler2D Texture;
uniform float u_time;
uniform vec2 u_resolution;
uniform float u_intensity;

vec3 palette(float t) {
    vec3 a = vec3(0.605, 0.592, 0.261);
    vec3 b = vec3(0.400, 0.690, 0.271);
    vec3 c = vec3(0.402, 0.645, 0.250);
    vec3 d = vec3(0.660, 0.220, 0.620);
    return a + b * cos(6.28318 * (c * t + d));
}

void main() {
    vec2 p = (uv - 0.5) * 2.0;
    p.x *= u_resolution.x / max(u_resolution.y, 1.0);

    float t = u_time * 1.030;
    float intensity = 0.2 + u_intensity * 1.35;

    float warpA = sin(p.y * 5.700 + t * 1.810) * 0.115;
    float warpB = cos(p.x * 2.630 - t * 1.210) * 0.144;
    vec2 q = p;
    q += vec2(warpA + warpB, sin((p.x + p.y) * 3.650 + t * 2.840) * 0.062) * intensity;

    float v = 0.0;
    v += sin(length(q) * 12.100 - t * 2.100);
    v += sin((q.x * 2.490 + q.y * 1.610) * 4.680 + t * 1.960);
    v += cos((q.x - q.y) * 2.890 - t * 1.830);
    v = 0.5 + 0.5 * (v / 3.0);

    float flicker = 0.5 + 0.5 * sin(t * 2.7 + v * 12.0 + q.x * 3.0);
    vec3 fx = palette(v + 0.12 * flicker);

    float vignette = pow(1.0 - smoothstep(0.2, 1.4, length(p)), 1.330);
    fx *= 0.35 + 0.65 * vignette;

    vec4 tex = texture2D(Texture, uv);
    gl_FragColor = vec4(fx * color.rgb, tex.a * color.a);
}

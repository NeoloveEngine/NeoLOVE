#version 100
precision mediump float;

varying lowp vec2 uv;
varying lowp vec4 color;

uniform sampler2D Texture;
uniform float u_time;
uniform vec2 u_resolution;
uniform float u_intensity;

vec3 palette(float t) {
    vec3 a = vec3(0.658, 0.289, 0.461);
    vec3 b = vec3(0.669, 0.457, 0.197);
    vec3 c = vec3(0.627, 0.445, 0.193);
    vec3 d = vec3(0.220, 0.490, 0.380);
    return a + b * cos(6.28318 * (c * t + d));
}

void main() {
    vec2 p = (uv - 0.5) * 2.0;
    p.x *= u_resolution.x / max(u_resolution.y, 1.0);

    float t = u_time * 0.710;
    float intensity = 0.2 + u_intensity * 1.35;

    float warpA = sin(p.y * 3.500 + t * 1.070) * 0.080;
    float warpB = cos(p.x * 4.520 - t * 2.140) * 0.088;
    vec2 q = p;
    q += vec2(warpA + warpB, sin((p.x + p.y) * 1.600 + t * 1.680) * 0.128) * intensity;

    float v = 0.0;
    v += sin(length(q) * 7.900 - t * 0.800);
    v += sin((q.x * 1.630 + q.y * 1.100) * 6.090 + t * 2.680);
    v += cos((q.x - q.y) * 3.880 - t * 2.460);
    v = 0.5 + 0.5 * (v / 3.0);

    float flicker = 0.5 + 0.5 * sin(t * 2.7 + v * 12.0 + q.x * 3.0);
    vec3 fx = palette(v + 0.12 * flicker);

    float vignette = pow(1.0 - smoothstep(0.2, 1.4, length(p)), 1.010);
    fx *= 0.35 + 0.65 * vignette;

    vec4 tex = texture2D(Texture, uv);
    gl_FragColor = vec4(fx * color.rgb, tex.a * color.a);
}

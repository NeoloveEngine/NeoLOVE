#version 100
precision mediump float;

varying lowp vec2 uv;
varying lowp vec4 color;

uniform sampler2D Texture;
uniform float u_time;
uniform vec2 u_resolution;
uniform float u_intensity;

vec3 palette(float t) {
    vec3 a = vec3(0.384, 0.700, 0.393);
    vec3 b = vec3(0.225, 0.662, 0.470);
    vec3 c = vec3(0.246, 0.643, 0.409);
    vec3 d = vec3(0.440, 0.580, 0.380);
    return a + b * cos(6.28318 * (c * t + d));
}

void main() {
    vec2 p = (uv - 0.5) * 2.0;
    p.x *= u_resolution.x / max(u_resolution.y, 1.0);

    float t = u_time * 0.710;
    float intensity = 0.2 + u_intensity * 1.35;

    float warpA = sin(p.y * 4.600 + t * 1.070) * 0.185;
    float warpB = cos(p.x * 5.150 - t * 2.450) * 0.088;
    vec2 q = p;
    q += vec2(warpA + warpB, sin((p.x + p.y) * 2.830 + t * 2.260) * 0.150) * intensity;

    float v = 0.0;
    v += sin(length(q) * 10.000 - t * 1.580);
    v += sin((q.x * 1.630 + q.y * 2.630) * 3.740 + t * 1.480);
    v += cos((q.x - q.y) * 2.230 - t * 1.410);
    v = 0.5 + 0.5 * (v / 3.0);

    float flicker = 0.5 + 0.5 * sin(t * 2.7 + v * 12.0 + q.x * 3.0);
    vec3 fx = palette(v + 0.12 * flicker);

    float vignette = pow(1.0 - smoothstep(0.2, 1.4, length(p)), 1.010);
    fx *= 0.35 + 0.65 * vignette;

    vec4 tex = texture2D(Texture, uv);
    gl_FragColor = vec4(fx * color.rgb, tex.a * color.a);
}

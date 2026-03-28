#version 100
precision mediump float;

varying lowp vec2 uv;
varying lowp vec4 color;

uniform sampler2D Texture;
uniform float u_time;
uniform vec2 u_resolution;
uniform float u_intensity;

vec3 palette(float t) {
    vec3 a = vec3(0.472, 0.284, 0.652);
    vec3 b = vec3(0.409, 0.218, 0.655);
    vec3 c = vec3(0.310, 0.272, 0.653);
    vec3 d = vec3(0.660, 0.130, 0.380);
    return a + b * cos(6.28318 * (c * t + d));
}

void main() {
    vec2 p = (uv - 0.5) * 2.0;
    p.x *= u_resolution.x / max(u_resolution.y, 1.0);

    float t = u_time * 0.710;
    float intensity = 0.2 + u_intensity * 1.35;

    float warpA = sin(p.y * 5.700 + t * 1.070) * 0.150;
    float warpB = cos(p.x * 2.000 - t * 0.900) * 0.088;
    vec2 q = p;
    q += vec2(warpA + warpB, sin((p.x + p.y) * 4.060 + t * 2.840) * 0.040) * intensity;

    float v = 0.0;
    v += sin(length(q) * 12.100 - t * 2.360);
    v += sin((q.x * 1.630 + q.y * 2.120) * 5.620 + t * 2.440);
    v += cos((q.x - q.y) * 3.880 - t * 2.460);
    v = 0.5 + 0.5 * (v / 3.0);

    float flicker = 0.5 + 0.5 * sin(t * 2.7 + v * 12.0 + q.x * 3.0);
    vec3 fx = palette(v + 0.12 * flicker);

    float vignette = pow(1.0 - smoothstep(0.2, 1.4, length(p)), 1.010);
    fx *= 0.35 + 0.65 * vignette;

    vec4 tex = texture2D(Texture, uv);
    gl_FragColor = vec4(fx * color.rgb, tex.a * color.a);
}

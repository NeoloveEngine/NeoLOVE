#version 100
precision mediump float;

varying lowp vec2 uv;
varying lowp vec4 color;

uniform sampler2D Texture;
uniform float u_time;
uniform vec2 u_resolution;
uniform float u_intensity;

vec3 palette(float t) {
    vec3 a = vec3(0.591, 0.606, 0.264);
    vec3 b = vec3(0.683, 0.422, 0.212);
    vec3 c = vec3(0.655, 0.279, 0.302);
    vec3 d = vec3(0.110, 0.220, 0.380);
    return a + b * cos(6.28318 * (c * t + d));
}

void main() {
    vec2 p = (uv - 0.5) * 2.0;
    p.x *= u_resolution.x / max(u_resolution.y, 1.0);

    float t = u_time * 0.710;
    float intensity = 0.2 + u_intensity * 1.35;

    float warpA = sin(p.y * 2.950 + t * 1.070) * 0.115;
    float warpB = cos(p.x * 2.630 - t * 1.210) * 0.088;
    vec2 q = p;
    q += vec2(warpA + warpB, sin((p.x + p.y) * 2.010 + t * 1.390) * 0.062) * intensity;

    float v = 0.0;
    v += sin(length(q) * 6.850 - t * 1.060);
    v += sin((q.x * 1.630 + q.y * 1.610) * 3.270 + t * 1.240);
    v += cos((q.x - q.y) * 2.230 - t * 1.410);
    v = 0.5 + 0.5 * (v / 3.0);

    float flicker = 0.5 + 0.5 * sin(t * 2.7 + v * 12.0 + q.x * 3.0);
    vec3 fx = palette(v + 0.12 * flicker);

    float vignette = pow(1.0 - smoothstep(0.2, 1.4, length(p)), 1.010);
    fx *= 0.35 + 0.65 * vignette;

    vec4 tex = texture2D(Texture, uv);
    gl_FragColor = vec4(fx * color.rgb, tex.a * color.a);
}

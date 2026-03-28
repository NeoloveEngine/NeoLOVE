#version 100
precision mediump float;

varying lowp vec2 uv;
varying lowp vec4 color;

uniform sampler2D Texture;
uniform float u_time;
uniform vec2 u_resolution;
uniform float u_intensity;

vec3 palette(float t) {
    vec3 a = vec3(0.260, 0.583, 0.614);
    vec3 b = vec3(0.186, 0.493, 0.649);
    vec3 c = vec3(0.180, 0.524, 0.574);
    vec3 d = vec3(0.220, 0.400, 0.740);
    return a + b * cos(6.28318 * (c * t + d));
}

void main() {
    vec2 p = (uv - 0.5) * 2.0;
    p.x *= u_resolution.x / max(u_resolution.y, 1.0);

    float t = u_time * 1.190;
    float intensity = 0.2 + u_intensity * 1.35;

    float warpA = sin(p.y * 3.500 + t * 2.180) * 0.115;
    float warpB = cos(p.x * 3.890 - t * 1.830) * 0.172;
    vec2 q = p;
    q += vec2(warpA + warpB, sin((p.x + p.y) * 2.010 + t * 1.680) * 0.106) * intensity;

    float v = 0.0;
    v += sin(length(q) * 7.900 - t * 1.060);
    v += sin((q.x * 2.920 + q.y * 1.610) * 2.800 + t * 1.000);
    v += cos((q.x - q.y) * 4.870 - t * 3.090);
    v = 0.5 + 0.5 * (v / 3.0);

    float flicker = 0.5 + 0.5 * sin(t * 2.7 + v * 12.0 + q.x * 3.0);
    vec3 fx = palette(v + 0.12 * flicker);

    float vignette = pow(1.0 - smoothstep(0.2, 1.4, length(p)), 1.490);
    fx *= 0.35 + 0.65 * vignette;

    vec4 tex = texture2D(Texture, uv);
    gl_FragColor = vec4(fx * color.rgb, tex.a * color.a);
}

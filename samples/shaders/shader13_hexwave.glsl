#version 100
precision mediump float;

varying lowp vec2 uv;
varying lowp vec4 color;

uniform sampler2D Texture;
uniform float u_time;
uniform vec2 u_resolution;

vec2 hexCoords(vec2 p) {
    const vec2 k = vec2(1.7320508, 1.0);
    p.x /= k.x;
    p *= 2.0;

    vec2 a = mod(p, k) - 0.5 * k;
    vec2 b = mod(p - 0.5 * k, k) - 0.5 * k;
    vec2 g = dot(a, a) < dot(b, b) ? a : b;
    return g;
}

void main() {
    vec2 p = (uv - 0.5) * 3.2;
    p.x *= u_resolution.x / max(u_resolution.y, 1.0);

    vec2 g = hexCoords(p);
    float d = length(g);

    float edge = 1.0 - smoothstep(0.18, 0.21, d);
    float wave = 0.5 + 0.5 * sin((p.x + p.y) * 4.0 - u_time * 2.4 + d * 20.0);

    vec3 bg = vec3(0.03, 0.05, 0.10);
    vec3 a = vec3(0.08, 0.65, 1.0);
    vec3 b = vec3(0.82, 0.32, 1.0);

    vec3 outColor = mix(bg, mix(a, b, wave), edge);

    vec4 tex = texture2D(Texture, uv);
    gl_FragColor = vec4(outColor * color.rgb, tex.a * color.a);
}

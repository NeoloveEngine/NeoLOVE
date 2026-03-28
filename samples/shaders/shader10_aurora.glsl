#version 100
precision mediump float;

varying lowp vec2 uv;
varying lowp vec4 color;

uniform sampler2D Texture;
uniform float u_time;
uniform vec2 u_resolution;

void main() {
    vec2 p = (uv - 0.5) * 2.0;
    p.x *= u_resolution.x / max(u_resolution.y, 1.0);

    float t = u_time * 0.45;
    float drift = sin(p.x * 2.0 + t) * 0.25 + sin(p.x * 4.6 - t * 1.6) * 0.12;
    float bandA = smoothstep(0.24, 0.0, abs(p.y + 0.25 + drift));
    float bandB = smoothstep(0.22, 0.0, abs(p.y - 0.02 + drift * 0.7));
    float bandC = smoothstep(0.2, 0.0, abs(p.y - 0.3 + drift * 0.45));

    vec3 base = vec3(0.03, 0.05, 0.12);
    vec3 c1 = vec3(0.10, 0.95, 0.62) * bandA;
    vec3 c2 = vec3(0.22, 0.55, 1.00) * bandB;
    vec3 c3 = vec3(0.70, 0.35, 1.00) * bandC;

    vec3 outColor = base + c1 + c2 + c3;
    outColor *= 0.95 + 0.05 * sin((p.x + p.y) * 40.0 + u_time * 2.5);

    vec4 tex = texture2D(Texture, uv);
    gl_FragColor = vec4(outColor * color.rgb, tex.a * color.a);
}

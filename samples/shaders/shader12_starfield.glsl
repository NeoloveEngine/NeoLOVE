#version 100
precision mediump float;

varying lowp vec2 uv;
varying lowp vec4 color;

uniform sampler2D Texture;
uniform float u_time;
uniform vec2 u_resolution;

float star(vec2 p, float flare) {
    float d = length(p);
    float m = 0.04 / max(d, 0.0008);
    float rays = max(0.0, 1.0 - abs(p.x * p.y * 900.0));
    m += rays * flare;
    m *= smoothstep(1.0, 0.2, d);
    return m;
}

float hash(vec2 p) {
    return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453123);
}

void main() {
    vec2 p = uv - 0.5;
    p.x *= u_resolution.x / max(u_resolution.y, 1.0);

    vec3 outColor = vec3(0.01, 0.02, 0.07);

    for (int i = 0; i < 3; i++) {
        float fi = float(i);
        vec2 lp = p * (2.0 + fi * 1.3);
        lp.y += u_time * (0.18 + fi * 0.12);
        vec2 cell = floor(lp);
        vec2 fracp = fract(lp) - 0.5;

        vec2 r = vec2(hash(cell), hash(cell + 13.7)) - 0.5;
        vec2 sp = fracp - r * 0.45;
        float s = star(sp, 0.02 + 0.02 * fi);

        vec3 c = vec3(0.4 + 0.3 * hash(cell + 1.0), 0.5 + 0.4 * hash(cell + 2.0), 1.0);
        outColor += s * c;
    }

    outColor = clamp(outColor, 0.0, 1.0);

    vec4 tex = texture2D(Texture, uv);
    gl_FragColor = vec4(outColor * color.rgb, tex.a * color.a);
}

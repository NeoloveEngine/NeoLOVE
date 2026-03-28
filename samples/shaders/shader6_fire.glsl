#version 100
precision mediump float;

varying lowp vec2 uv;
varying lowp vec4 color;

uniform sampler2D Texture;
uniform float u_time;
uniform vec2 u_resolution;

float flameField(vec2 p, float t) {
    float v = 0.0;
    float a = 0.5;

    for (int i = 0; i < 4; i++) {
        v += a * (0.5 + 0.5 * sin(p.x) * cos(p.y));
        p = mat2(1.6, -1.2, 1.2, 1.6) * p + vec2(0.3, 1.1 + 0.2 * t);
        a *= 0.55;
    }

    return v;
}

void main() {
    float t = u_time * 1.25;
    vec2 p = vec2((uv.x - 0.5) * 3.2, uv.y * 3.8 - t * 1.8);

    float f = flameField(p, t);
    float body = smoothstep(1.05, 0.12, abs(uv.x - 0.5) * (1.25 - uv.y));
    float flame = smoothstep(0.18, 1.12, f + (1.0 - uv.y) * 0.95) * body;

    vec3 dark = vec3(0.06, 0.01, 0.00);
    vec3 hot = vec3(1.00, 0.30, 0.03);
    vec3 core = vec3(1.00, 0.92, 0.46);

    vec3 outColor = mix(dark, hot, flame);
    outColor = mix(outColor, core, smoothstep(0.62, 1.0, flame));

    vec4 tex = texture2D(Texture, uv);
    gl_FragColor = vec4(outColor * color.rgb, tex.a * color.a);
}

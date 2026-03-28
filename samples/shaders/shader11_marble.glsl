#version 100
precision mediump float;

varying lowp vec2 uv;
varying lowp vec4 color;

uniform sampler2D Texture;
uniform float u_time;
uniform vec2 u_resolution;

void main() {
    vec2 p = (uv - 0.5) * 4.0;
    p.x *= u_resolution.x / max(u_resolution.y, 1.0);

    float t = u_time * 0.7;
    float v = sin(p.x + sin(p.y * 1.8 + t) * 1.8 + sin(p.x * 1.4 - t * 1.2));
    v += 0.45 * sin((p.x + p.y) * 1.7 - t * 1.5);
    v = 0.5 + 0.5 * (v / 1.45);

    vec3 dark = vec3(0.12, 0.09, 0.08);
    vec3 mid = vec3(0.68, 0.63, 0.56);
    vec3 light = vec3(0.96, 0.93, 0.88);

    vec3 outColor = mix(dark, mid, smoothstep(0.18, 0.72, v));
    outColor = mix(outColor, light, smoothstep(0.68, 1.0, v));

    vec4 tex = texture2D(Texture, uv);
    gl_FragColor = vec4(outColor * color.rgb, tex.a * color.a);
}

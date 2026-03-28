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

    float t = u_time * 0.8;

    vec2 q = p;
    q += 0.28 * vec2(sin(1.7 * q.y + t), cos(1.3 * q.x - t));
    q += 0.18 * vec2(cos(2.4 * q.y - 1.3 * t), sin(2.0 * q.x + 1.1 * t));
    q += 0.12 * vec2(sin(3.1 * q.y + 0.6 * t), cos(2.7 * q.x - 0.8 * t));

    float f = 0.0;
    f += sin(3.2 * q.x + 2.1 * q.y + t * 1.2);
    f += sin(4.0 * q.y - 2.6 * q.x - t * 0.9);
    f += sin(2.5 * length(q) - t * 1.5);

    float v = 0.5 + 0.5 * (f / 3.0);

    vec3 deep = vec3(0.01, 0.08, 0.18);
    vec3 mid = vec3(0.03, 0.55, 0.82);
    vec3 light = vec3(0.83, 0.97, 1.0);

    vec3 outColor = mix(deep, mid, smoothstep(0.15, 0.75, v));
    outColor = mix(outColor, light, smoothstep(0.68, 1.0, v));

    vec4 tex = texture2D(Texture, uv);
    gl_FragColor = vec4(outColor * color.rgb, tex.a * color.a);
}

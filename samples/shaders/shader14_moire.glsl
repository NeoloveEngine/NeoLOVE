#version 100
precision mediump float;

varying lowp vec2 uv;
varying lowp vec4 color;

uniform sampler2D Texture;
uniform float u_time;
uniform vec2 u_resolution;

void main() {
    vec2 p = (uv - 0.5) * 2.4;
    p.x *= u_resolution.x / max(u_resolution.y, 1.0);

    float r = length(p);
    float a = atan(p.y, p.x);

    float rings = sin(r * 80.0 - u_time * 8.0);
    float spokes = sin(a * 42.0 + u_time * 3.0);
    float moire = rings * 0.65 + spokes * 0.35;
    float v = 0.5 + 0.5 * moire;

    vec3 c1 = vec3(0.04, 0.08, 0.16);
    vec3 c2 = vec3(0.10, 0.95, 0.90);
    vec3 c3 = vec3(0.95, 0.30, 0.95);

    vec3 outColor = mix(c1, c2, smoothstep(0.2, 0.75, v));
    outColor = mix(outColor, c3, smoothstep(0.72, 1.0, v));

    vec4 tex = texture2D(Texture, uv);
    gl_FragColor = vec4(outColor * color.rgb, tex.a * color.a);
}

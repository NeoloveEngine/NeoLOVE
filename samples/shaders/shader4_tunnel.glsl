#version 100
precision mediump float;

varying lowp vec2 uv;
varying lowp vec4 color;

uniform sampler2D Texture;
uniform float u_time;
uniform vec2 u_resolution;

void main() {
    vec2 p = uv - 0.5;
    p.x *= u_resolution.x / max(u_resolution.y, 1.0);

    float r = length(p);
    float a = atan(p.y, p.x);

    float rings = sin(24.0 * r - u_time * 7.0);
    float spokes = sin(a * 10.0 + u_time * 1.8);
    float v = 0.5 + 0.5 * (rings * 0.75 + spokes * 0.25);

    vec3 nearColor = vec3(0.92, 0.35, 1.0);
    vec3 farColor = vec3(0.02, 0.04, 0.12);
    vec3 outColor = mix(farColor, nearColor, v);

    float fade = 1.0 - smoothstep(0.2, 1.15, r);
    outColor *= fade + 0.15;

    vec4 tex = texture2D(Texture, uv);
    gl_FragColor = vec4(outColor * color.rgb, tex.a * color.a);
}

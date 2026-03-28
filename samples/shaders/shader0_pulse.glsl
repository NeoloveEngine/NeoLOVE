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

    float rings = sin(length(p) * 16.0 - u_time * 4.0);
    float grid = sin((uv.x + uv.y + u_time * 0.15) * 20.0);
    float v = 0.5 + 0.5 * (rings * 0.75 + grid * 0.25);

    vec3 cA = vec3(0.03, 0.10, 0.24);
    vec3 cB = vec3(0.15, 0.75, 1.00);
    vec3 cC = vec3(1.00, 0.90, 0.55);

    vec3 outColor = mix(cA, cB, smoothstep(0.1, 0.8, v));
    outColor = mix(outColor, cC, smoothstep(0.78, 1.0, v));

    vec4 tex = texture2D(Texture, uv);
    gl_FragColor = vec4(outColor * color.rgb, tex.a * color.a);
}

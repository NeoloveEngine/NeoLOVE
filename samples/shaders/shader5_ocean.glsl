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

    float waveA = sin((p.x * 2.8 + u_time * 0.9) * 2.2 + sin(p.y * 2.3 + u_time * 0.4));
    float waveB = sin((p.x * 5.1 - u_time * 1.4) + cos(p.y * 3.7 - u_time * 0.25));
    float h = 0.5 + 0.5 * (waveA * 0.6 + waveB * 0.4);

    vec3 deep = vec3(0.02, 0.09, 0.20);
    vec3 mid = vec3(0.04, 0.38, 0.68);
    vec3 foam = vec3(0.78, 0.92, 1.0);

    vec3 outColor = mix(deep, mid, h);
    float crest = smoothstep(0.72, 0.95, h);
    outColor = mix(outColor, foam, crest * 0.75);

    vec4 tex = texture2D(Texture, uv);
    gl_FragColor = vec4(outColor * color.rgb, tex.a * color.a);
}

#version 100
precision mediump float;

varying lowp vec2 uv;
varying lowp vec4 color;

uniform sampler2D Texture;
uniform float u_time;
uniform vec2 u_resolution;

void main() {
    vec2 p = uv;
    float v = 0.0;

    v += sin((p.x + u_time * 0.12) * 13.0);
    v += sin((p.y - u_time * 0.15) * 12.0);
    v += sin((p.x + p.y + u_time * 0.09) * 16.0);
    v += sin((length((p - 0.5) * 2.0) - u_time * 0.3) * 18.0);
    v = (v / 4.0) * 0.5 + 0.5;

    vec3 outColor = 0.5 + 0.5 * cos(vec3(0.0, 2.1, 4.2) + v * 6.28318 + u_time * vec3(0.25, 0.18, 0.14));

    vec4 tex = texture2D(Texture, uv);
    gl_FragColor = vec4(outColor * color.rgb, tex.a * color.a);
}

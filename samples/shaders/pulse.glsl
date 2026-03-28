#version 100
precision lowp float;

varying lowp vec2 uv;
varying lowp vec4 color;

uniform sampler2D Texture;
uniform float u_time;

void main() {
    vec4 base = texture2D(Texture, uv) * color;
    float pulse = 0.55 + 0.45 * sin(u_time * 2.4 + uv.x * 14.0 + uv.y * 8.0);
    gl_FragColor = vec4(base.rgb * pulse, base.a);
}

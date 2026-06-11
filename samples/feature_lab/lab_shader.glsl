#version 100
precision lowp float;

varying lowp vec2 uv;
varying lowp vec4 color;

uniform sampler2D Texture;
uniform sampler2D u_extra;
uniform float u_time;
uniform vec2 u_resolution;
uniform vec3 u_tint3;
uniform vec4 u_tint4;
uniform vec4 u_color;

void main() {
    vec4 base = texture2D(Texture, uv) * color;
    vec4 extra = texture2D(u_extra, uv);
    float wave = 0.62 + 0.38 * sin(u_time * 2.4 + uv.x * 13.0 + uv.y * 9.0);
    vec3 tint = mix(u_tint3, u_tint4.rgb, 0.5) * u_color.rgb;
    gl_FragColor = vec4(mix(base.rgb, extra.rgb, 0.18) * tint * wave, base.a);
}

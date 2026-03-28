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

    float a = atan(p.y, p.x);
    float r = length(p);

    float segments = 8.0;
    float segmentAngle = 6.28318 / segments;
    a = mod(a, segmentAngle);
    a = abs(a - 0.5 * segmentAngle);

    vec2 q = vec2(cos(a), sin(a)) * r;
    float w = sin(15.0 * q.x + u_time * 2.2) * cos(13.0 * q.y - u_time * 1.4);
    float v = 0.5 + 0.5 * w;

    vec3 outColor = 0.5 + 0.5 * cos(vec3(0.0, 2.2, 4.0) + v * 6.28318 + u_time * 0.35);
    outColor *= 1.0 - smoothstep(0.75, 1.2, r);

    vec4 tex = texture2D(Texture, uv);
    gl_FragColor = vec4(outColor * color.rgb, tex.a * color.a);
}

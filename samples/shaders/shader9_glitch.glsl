#version 100
precision mediump float;

varying lowp vec2 uv;
varying lowp vec4 color;

uniform sampler2D Texture;
uniform float u_time;
uniform vec2 u_resolution;

vec3 basePattern(vec2 p, float t) {
    float wave = sin((p.x * 8.0 + p.y * 12.0) - t * 2.2);
    float ring = sin(length((p - 0.5) * vec2(1.5, 1.0)) * 22.0 - t * 3.0);
    float v = 0.5 + 0.5 * (0.6 * wave + 0.4 * ring);
    return 0.5 + 0.5 * cos(vec3(0.0, 2.0, 4.0) + v * 6.28318 + t * 0.2);
}

void main() {
    float t = u_time;

    float row = floor(uv.y * 48.0);
    float pulse = 0.5 + 0.5 * sin(row * 2.7 + floor(t * 12.0));
    float block = step(0.78, pulse);

    float shift = sin(uv.y * 280.0 + t * 11.0) * 0.005;
    shift += block * 0.035 * sin(t * 4.0 + row * 0.6);

    vec3 r = basePattern(uv + vec2(shift, 0.0), t);
    vec3 g = basePattern(uv, t);
    vec3 b = basePattern(uv - vec2(shift, 0.0), t);

    float scan = 0.92 + 0.08 * sin((uv.y + t * 0.4) * 420.0);
    vec3 outColor = vec3(r.r, g.g, b.b) * scan;

    vec4 tex = texture2D(Texture, uv);
    gl_FragColor = vec4(outColor * color.rgb, tex.a * color.a);
}

#version 100
precision mediump float;

varying lowp vec2 uv;
varying lowp vec4 color;

uniform sampler2D Texture;
uniform float u_time;
uniform vec2 u_resolution;

void main() {
    vec2 p = (uv - 0.5) * 3.2;
    p.x *= u_resolution.x / max(u_resolution.y, 1.0);

    float t = u_time * 0.9;

    vec2 q = p;
    q += 0.30 * vec2(sin(1.8 * q.y + t), cos(1.4 * q.x - 1.2 * t));
    q += 0.15 * vec2(cos(2.5 * q.y - 0.7 * t), sin(2.1 * q.x + 0.9 * t));

    float veins = abs(sin(3.6 * q.x + 4.2 * q.y + sin(2.0 * q.x - 1.8 * q.y + t)));
    float crack = pow(1.0 - veins, 4.0);

    float rockNoise = 0.5 + 0.5 * sin(2.4 * p.x - 1.7 * p.y + t * 0.3);
    vec3 rock = mix(vec3(0.05, 0.02, 0.02), vec3(0.18, 0.08, 0.05), rockNoise);

    vec3 lavaA = vec3(1.00, 0.26, 0.02);
    vec3 lavaB = vec3(1.00, 0.74, 0.20);
    vec3 lava = mix(lavaA, lavaB, 0.5 + 0.5 * sin(t * 2.0 + q.x * 4.0));

    vec3 outColor = mix(rock, lava, crack);
    outColor += crack * 0.18;

    vec4 tex = texture2D(Texture, uv);
    gl_FragColor = vec4(outColor * color.rgb, tex.a * color.a);
}

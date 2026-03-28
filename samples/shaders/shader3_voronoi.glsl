#version 100
precision mediump float;

varying lowp vec2 uv;
varying lowp vec4 color;

uniform sampler2D Texture;
uniform float u_time;
uniform vec2 u_resolution;

vec2 rand2(vec2 p) {
    return 0.5 + 0.5 * sin(vec2(
        dot(p, vec2(2.1, 1.7)),
        dot(p, vec2(1.3, 2.9))
    ));
}

void main() {
    vec2 p = uv * 7.5;
    vec2 cell = floor(p);
    vec2 f = fract(p);

    float minDist = 10.0;
    float secondDist = 10.0;

    for (int y = -1; y <= 1; y++) {
        for (int x = -1; x <= 1; x++) {
            vec2 o = vec2(float(x), float(y));
            vec2 id = cell + o;
            vec2 r = rand2(id);
            vec2 site = 0.5 + 0.38 * sin(u_time * 0.85 + 6.28318 * r);

            vec2 d = o + site - f;
            float dist2 = dot(d, d);

            if (dist2 < minDist) {
                secondDist = minDist;
                minDist = dist2;
            } else if (dist2 < secondDist) {
                secondDist = dist2;
            }
        }
    }

    float edge = sqrt(secondDist) - sqrt(minDist);
    float border = 1.0 - smoothstep(0.03, 0.08, edge);
    float fill = smoothstep(0.02, 0.22, sqrt(minDist));

    vec3 bg = vec3(0.02, 0.04, 0.10);
    vec3 cellColor = vec3(0.14, 0.74, 0.98);
    vec3 borderColor = vec3(0.85, 0.98, 1.0);

    vec3 outColor = mix(bg, cellColor, fill);
    outColor = mix(outColor, borderColor, border * 0.9);

    vec4 tex = texture2D(Texture, uv);
    gl_FragColor = vec4(outColor * color.rgb, tex.a * color.a);
}

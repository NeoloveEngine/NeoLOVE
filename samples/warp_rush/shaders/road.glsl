#version 100
precision mediump float;

varying lowp vec2 uv;
varying lowp vec4 color;

uniform sampler2D Texture;
uniform float u_time;
uniform vec2 u_resolution;
uniform float u_intensity;
uniform float u_scroll;
uniform float u_center_near;
uniform float u_center_far;
uniform float u_width_near;
uniform float u_width_far;
uniform float u_wave;

void main() {
    float horizon = 0.18;
    float r = (uv.y - horizon) / (1.0 - horizon);
    if (r <= 0.0) {
        gl_FragColor = vec4(0.0, 0.0, 0.0, 0.0);
        return;
    }
    r = clamp(r, 0.0, 1.0);

    float nearK = pow(r, 1.12);
    float farK = 1.0 - nearK;
    float width = mix(u_width_far, u_width_near, nearK);
    float center = mix(u_center_far, u_center_near, nearK);

    // Slight depth wave over the far road section to keep motion alive.
    float z = 1.0 / (r + 0.07);
    float waveA = sin(z * 0.92 - u_scroll * 0.72 + u_time * 0.55);
    float waveB = sin(z * 1.67 - u_scroll * 0.35 + u_time * 0.31);
    center += (waveA * 0.65 + waveB * 0.35) * u_wave * farK;
    center = clamp(center, width + 0.03, 1.0 - width - 0.03);

    float dx = abs(uv.x - center);
    float edgeSoft = 0.004 + 0.006 * (1.0 - r);
    float roadMask = 1.0 - smoothstep(width - edgeSoft, width + edgeSoft, dx);
    if (roadMask <= 0.001) {
        gl_FragColor = vec4(0.0, 0.0, 0.0, 0.0);
        return;
    }

    float stripe = 0.5 + 0.5 * sin(z * 2.8 - u_scroll * 1.9);

    vec3 dark = vec3(0.08, 0.14, 0.24);
    vec3 light = vec3(0.18, 0.32, 0.50);
    vec3 roadColor = mix(dark, light, stripe);

    float railOuter = smoothstep(width - 0.014, width - 0.003, dx);
    float railInner = smoothstep(width - 0.003, width + 0.002, dx);
    float rail = clamp(railOuter - railInner, 0.0, 1.0);

    float laneCoord = abs((uv.x - center) / max(width, 0.0001));
    float laneLine = 1.0 - smoothstep(0.0, 0.028, laneCoord);
    float dash = step(0.58, fract(z * 0.35 - u_scroll * 0.12));
    float laneMask = laneLine * dash;

    vec3 railColor = vec3(0.90, 0.95, 1.0);
    vec3 laneColor = vec3(0.95, 0.98, 1.0);

    vec3 outColor = roadColor;
    outColor = mix(outColor, railColor, rail * (0.55 + 0.45 * (1.0 - r)));
    outColor = mix(outColor, laneColor, laneMask * (0.5 + 0.5 * r));

    float shade = 0.82 + 0.18 * r;
    outColor *= shade;

    gl_FragColor = vec4(outColor * color.rgb, roadMask * color.a);
}

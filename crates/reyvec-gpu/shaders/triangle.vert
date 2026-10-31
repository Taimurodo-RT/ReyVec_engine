#version 450
// Stage 1 test triangle. Vertices are built into the shader, no buffers.
// Engine convention: Y points up, so we flip Y for Vulkan clip space.

// Push constants are shared by all engine pipelines (128 bytes, all stages).
layout(push_constant) uniform Push {
    float time;   // seconds since start
    float aspect; // width / height, keeps the triangle from stretching
} pc;

layout(location = 0) out vec3 v_color;

const vec2 POSITIONS[3] = vec2[](
    vec2(0.0, 0.6),
    vec2(-0.52, -0.3),
    vec2(0.52, -0.3)
);

const vec3 COLORS[3] = vec3[](
    vec3(1.0, 0.25, 0.2),
    vec3(0.2, 1.0, 0.35),
    vec3(0.25, 0.4, 1.0)
);

void main() {
    float c = cos(pc.time * 0.5);
    float s = sin(pc.time * 0.5);
    vec2 p = mat2(c, s, -s, c) * POSITIONS[gl_VertexIndex];
    p.x /= pc.aspect;
    gl_Position = vec4(p.x, -p.y, 0.0, 1.0);
    v_color = COLORS[gl_VertexIndex];
}

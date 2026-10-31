#version 450
// Textured sprites. Sprite data is read from a GPU buffer by address
// (buffer device address); textures come from the shared bindless list.
#extension GL_EXT_buffer_reference : require

struct Sprite {
    vec2 pos;       // centre, screen units: y in [-1, 1], x in [-aspect, aspect]
    vec2 size;      // width, height in the same units
    vec4 tint;      // colour multiplier, linear
    float rotation; // radians, counter-clockwise
    uint texture;   // index in textures[]
    uint sampler_id;// 0 = linear, 1 = nearest
    uint pad;
};

layout(buffer_reference, std430, buffer_reference_align = 16) readonly buffer Sprites {
    Sprite items[];
};

layout(push_constant) uniform Push {
    Sprites sprites;
    float aspect;
} pc;

layout(location = 0) out vec2 v_uv;
layout(location = 1) out vec4 v_tint;
layout(location = 2) flat out uint v_texture;
layout(location = 3) flat out uint v_sampler;

const vec2 CORNERS[6] = vec2[](
    vec2(-0.5, -0.5), vec2(0.5, -0.5), vec2(0.5, 0.5),
    vec2(-0.5, -0.5), vec2(0.5, 0.5), vec2(-0.5, 0.5)
);

void main() {
    Sprite s = pc.sprites.items[gl_InstanceIndex];
    vec2 corner = CORNERS[gl_VertexIndex];
    float c = cos(s.rotation);
    float n = sin(s.rotation);
    vec2 p = s.pos + mat2(c, n, -n, c) * (corner * s.size);
    p.x /= pc.aspect;
    gl_Position = vec4(p.x, -p.y, 0.0, 1.0);
    v_uv = vec2(corner.x + 0.5, 0.5 - corner.y);
    v_tint = s.tint;
    v_texture = s.texture;
    v_sampler = s.sampler_id;
}

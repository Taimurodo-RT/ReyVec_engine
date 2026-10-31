#version 450
#extension GL_EXT_nonuniform_qualifier : require

layout(set = 0, binding = 0) uniform texture2D textures[];
layout(set = 0, binding = 1) uniform sampler samplers[2];

layout(location = 0) in vec2 v_uv;
layout(location = 1) in vec4 v_tint;
layout(location = 2) flat in uint v_texture;
layout(location = 3) flat in uint v_sampler;

layout(location = 0) out vec4 out_color;

void main() {
    vec4 c;
    if (v_sampler == 1u) {
        c = texture(sampler2D(textures[nonuniformEXT(v_texture)], samplers[1]), v_uv);
    } else {
        c = texture(sampler2D(textures[nonuniformEXT(v_texture)], samplers[0]), v_uv);
    }
    out_color = c * v_tint;
}

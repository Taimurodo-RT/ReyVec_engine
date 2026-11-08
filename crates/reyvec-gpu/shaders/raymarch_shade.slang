// Сцена «3D», освещение в проходе «сцена» (этапы 7.3–7.4): читает буфер геометрии
// (raymarch.slang) и тени (raymarch_shadow.slang) и красит точку: материал, сетка пола,
// солнце с мягкой тенью, свет неба с затенением в углах, лампы, свечение, блик и дымка.

#include "include/space.slang"
#include "include/shade3d.slang"

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    int2 px = int2(input.position.xy);
    // Края форм (7.6о): цвет из нескольких лучей внутри пикселя уже посчитан.
    if ((pc.flags & SPACE_FLAG_EDGES) != 0)
    {
        float4 edge = textures[pc.target].Load(int3(px, 0));
        if (edge.a > 0.0)
            return float4(edge.rgb, 1.0);
    }
    float4 g = textures[pc.gbuffer].Load(int3(px, 0));
    return float4(shade_point(input.screen, g, px), 1.0);
}

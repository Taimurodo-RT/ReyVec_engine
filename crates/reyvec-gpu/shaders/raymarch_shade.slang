// Сцена «3D», освещение в проходе «сцена» (этапы 7.3–7.4): читает буфер геометрии
// (raymarch.slang) и тени (raymarch_shadow.slang) и красит точку: материал, сетка пола,
// солнце с мягкой тенью, свет неба с затенением в углах, лампы, свечение, блик и дымка.
// Прямой вывод (7.20а): при разрешении 3D 100 % и кадре, где нет ничего, кроме «3D», этот
// же шейдер красит прямо в окно; тогда стоит SPACE_FLAG_DITHER (шум как в present.slang).

#include "include/space.slang"
#include "include/shade3d.slang"
#include "include/dither.slang"

float3 shade_pixel(VertexOut input)
{
    int2 px = int2(input.position.xy);
    // Края форм (7.6о): цвет из нескольких лучей внутри пикселя уже посчитан.
    if ((pc.flags & SPACE_FLAG_EDGES) != 0)
    {
        float4 edge = textures[pc.target].Load(int3(px, 0));
        if (edge.a > 0.0)
            return edge.rgb;
    }
    float4 g = textures[pc.gbuffer].Load(int3(px, 0));
    return shade_point(input.screen, g, px);
}

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    float3 c = shade_pixel(input);
    if ((pc.flags & SPACE_FLAG_DITHER) != 0)
        c = dither(c, input.position.xy);
    return float4(c, 1.0);
}

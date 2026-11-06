// Сцена «3D», проход «края 3D» (7.6о): сглаживание краёв форм. Для каждого пикселя из
// списка (raymarch_edge_find.slang) четыре потока пускают по лучу через разные точки
// пикселя (повёрнутая решётка 4×: у всех лучей разные x и y, поэтому и вертикальные, и
// горизонтальные края получают четыре ступени), шагают и освещают их как обычно, а
// среднее пишут в картинку краёв. «Сцена» берёт его вместо цвета одного луча.
//
// Конус квадрата 8×8 (7.8) накрывает весь квадрат, поэтому любой луч внутри пикселя
// может начинать с его длины; полигоны берутся из самого пикселя.

[[vk::binding(0, 0)]] Texture2D textures[];
[[vk::binding(2, 0)]] [format("rgba16f")] RWTexture2D<float4> images[];

#include "include/march.slang"
#include "include/shade3d.slang"

// Файл обязан иметь пару для рисования (build.rs собирает её у всех шейдеров); здесь
// она не используется.
[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    return float4(0.0, 0.0, 0.0, 0.0);
}

// Сдвиги лучей от центра пикселя (в пикселях).
static const float2 SUBPIXEL[4] = {
    float2(-0.125, -0.375),
    float2(0.375, -0.125),
    float2(0.125, 0.375),
    float2(-0.375, 0.125),
};

groupshared float3 colors[64];

[shader("compute")]
[numthreads(64, 1, 1)]
void cs_main(uint3 id : SV_DispatchThreadID, uint lane : SV_GroupIndex)
{
    uint count = min(pc.edges[3], 65535 * 16); // как MAX_EDGES в raymarch_edge_find.slang
    uint i = id.x >> 2;
    uint sub = id.x & 3;
    int2 px = int2(0, 0);
    float3 color = float3(0.0, 0.0, 0.0);
    if (i < count)
    {
        uint packed = pc.edges[4 + i];
        px = int2(packed & 0xffff, packed >> 16);
        uint w, h;
        images[pc.target].GetDimensions(w, h);
        float2 at = float2(px) + 0.5 + SUBPIXEL[sub];
        float2 screen = float2(at.x / float(w) * 2.0 - 1.0, 1.0 - at.y / float(h) * 2.0);
        color = shade_point(screen, march_pixel(px, screen), px);
    }
    // Четыре луча пикселя — соседние потоки одной группы: складывает первый.
    colors[lane] = color;
    GroupMemoryBarrierWithGroupSync();
    if (i < count && sub == 0)
    {
        float3 sum = colors[lane] + colors[lane + 1] + colors[lane + 2] + colors[lane + 3];
        images[pc.target][px] = float4(sum * 0.25, 1.0);
    }
}

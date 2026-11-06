// Сцена «3D», проход «края 3D» (7.6о): сглаживание краёв форм. Для каждого пикселя из
// списка (raymarch_edge_find.slang) четыре потока пускают по лучу через разные точки
// пикселя (повёрнутая решётка 4×: у всех лучей разные x и y, поэтому и вертикальные, и
// горизонтальные края получают четыре ступени), шагают и освещают их как обычно, а
// среднее пишут в картинку краёв. «Сцена» берёт его вместо цвета одного луча.
//
// Конус квадрата 8×8 (7.8) накрывает весь квадрат, поэтому любой луч внутри пикселя
// может начинать с его длины; полигоны берутся из самого пикселя.
//
// Тени на краях считаются точно, для своей точки пикселя: картинка теней вдвое меньше
// кадра, и на перемычке тоньше двух пикселей или на полоске пола у края формы пиксель
// получал бы тень соседней поверхности. Четыре доли света (солнце, углы, две лампы)
// считают те же четыре потока пикселя, по одной. Луч, попавший на ту же поверхность,
// берёт эти точные доли, попавший на другую — растянутые из картинки теней. Точные
// доли считаются, только если растянутые для точки пикселя ненадёжны (7.6с).

[[vk::binding(0, 0)]] Texture2D textures[];
[[vk::binding(2, 0)]] [format("rgba16f")] RWTexture2D<float4> images[];

#include "include/march.slang"
#include "include/shade3d.slang"
#include "include/shadow3d.slang"

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
groupshared float parts[64];

[shader("compute")]
[numthreads(64, 1, 1)]
void cs_main(uint3 id : SV_DispatchThreadID, uint lane : SV_GroupIndex)
{
    uint count = min(pc.edges[3], 65535 * 16); // как MAX_EDGES в raymarch_edge_find.slang
    uint i = id.x >> 2;
    uint sub = id.x & 3;
    int2 px = int2(0, 0);
    uint w, h;
    images[pc.target].GetDimensions(w, h);
    float4 g0 = float4(-1.0, 0.0, 0.0, 0.0);
    if (i < count)
    {
        uint packed = pc.edges[4 + i];
        px = int2(packed & 0xffff, packed >> 16);
        g0 = textures[pc.gbuffer].Load(int3(px, 0));
    }

    // Точные доли света точки самого пикселя: каждый поток свою. Только там, где тени из
    // половинного размера для неё ненадёжны (7.6с): у силуэта большой формы её сторона
    // края и так покрыта похожими точками, и четыре луча тени на пиксель не нужны.
    float part = 1.0;
    float3 n0 = oct_decode(g0.zw);
    float similar = 0.0;
    float4 upsampled = float4(1.0, 1.0, 1.0, 1.0);
    if (g0.x >= 0.0)
        upsampled = upsampled_shadows_checked(px, g0.x, n0, similar);
    bool reliable = similar >= 0.5;
    if (g0.x >= 0.0 && !reliable)
    {
        float2 c = float2(px) + 0.5;
        Ray r0 = screen_ray(float2(c.x / float(w) * 2.0 - 1.0, 1.0 - c.y / float(h) * 2.0));
        part = shadow_part(r0.origin + r0.dir * g0.x, n0, g0.x, sub);
    }
    parts[lane] = part;
    GroupMemoryBarrierWithGroupSync();
    uint first = lane & ~3u;
    float4 exact = reliable ? upsampled
        : float4(parts[first], parts[first + 1], parts[first + 2], parts[first + 3]);

    float3 color = float3(0.0, 0.0, 0.0);
    if (i < count)
    {
        float2 at = float2(px) + 0.5 + SUBPIXEL[sub];
        float2 screen = float2(at.x / float(w) * 2.0 - 1.0, 1.0 - at.y / float(h) * 2.0);
        float4 g = march_pixel(px, screen);
        if (g.x < 0.0)
            color = sky(screen_ray(screen).dir);
        else
        {
            float3 n = oct_decode(g.zw);
            // Та же поверхность, что у точки пикселя: тот же материал, глубина и сторона.
            bool same = g0.x >= 0.0 && abs(g.y - g0.y) < 0.5
                && abs(g.x - g0.x) < 0.02 * g0.x + 0.05 && dot(n, n0) > 0.9;
            float4 shadows = same ? exact : upsampled_shadows(px, g.x, n);
            color = shade_lit(screen, g, shadows);
        }
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

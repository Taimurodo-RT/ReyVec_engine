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
// Два луча (SPACE_FLAG_EDGE_PAIRS, 7.15): точки MSAA 2× — по диагонали, у них разные x и
// y, и вертикальные, и горизонтальные края получают по две ступени.
static const float2 SUBPIXEL_PAIR[2] = {
    float2(0.25, 0.25),
    float2(-0.25, -0.25),
};

groupshared float3 colors[64];
groupshared float parts[64][2];

[shader("compute")]
[numthreads(64, 1, 1)]
void cs_main(uint3 id : SV_DispatchThreadID, uint lane : SV_GroupIndex)
{
    // Лучей на пиксель: 4 или 2. Лучи пикселя — соседние потоки одной группы.
    bool pairs = (pc.flags & SPACE_FLAG_EDGE_PAIRS) != 0;
    uint shift = pairs ? 1 : 2;
    uint rays = 1u << shift;
    uint count = min(pc.edges[3], 65535 * 16); // как MAX_EDGES в raymarch_edge_find.slang
    uint i = id.x >> shift;
    uint sub = id.x & (rays - 1);
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

    // Точные доли света точки самого пикселя (солнце, углы, две лампы): потоки пикселя
    // делят их между собой (при двух лучах — по две). Только там, где тени из половинного
    // размера для неё ненадёжны (7.6с): у силуэта большой формы её сторона края и так
    // покрыта похожими точками, и четыре луча тени на пиксель не нужны.
    float3 n0 = oct_decode(g0.zw);
    float similar = 0.0;
    float4 upsampled = float4(1.0, 1.0, 1.0, 1.0);
    if (g0.x >= 0.0)
        upsampled = upsampled_shadows_checked(px, g0.x, n0, similar);
    bool reliable = similar >= 0.5;
    float2 part = float2(1.0, 1.0);
    if (g0.x >= 0.0 && !reliable)
    {
        float2 c = float2(px) + 0.5;
        Ray r0 = screen_ray(float2(c.x / float(w) * 2.0 - 1.0, 1.0 - c.y / float(h) * 2.0));
        float3 surface = r0.origin + r0.dir * g0.x;
        part.x = shadow_part(surface, n0, g0.x, sub);
        if (pairs)
            part.y = shadow_part(surface, n0, g0.x, sub + 2);
    }
    parts[lane][0] = part.x;
    parts[lane][1] = part.y;
    GroupMemoryBarrierWithGroupSync();
    uint first = lane & ~(rays - 1);
    float4 exact = upsampled;
    if (!reliable)
        exact = pairs
            ? float4(parts[first][0], parts[first + 1][0], parts[first][1], parts[first + 1][1])
            : float4(parts[first][0], parts[first + 1][0], parts[first + 2][0], parts[first + 3][0]);

    float3 color = float3(0.0, 0.0, 0.0);
    if (i < count)
    {
        float2 at = float2(px) + 0.5 + (pairs ? SUBPIXEL_PAIR[sub] : SUBPIXEL[sub]);
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
    // Складывает первый поток пикселя.
    colors[lane] = color;
    GroupMemoryBarrierWithGroupSync();
    if (i < count && sub == 0)
    {
        float3 sum = colors[lane] + colors[lane + 1];
        if (!pairs)
            sum += colors[lane + 2] + colors[lane + 3];
        images[pc.target][px] = float4(sum / float(rays), 1.0);
    }
}

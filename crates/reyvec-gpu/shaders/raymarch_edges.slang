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
// делят потоки группы: каждый считает одну долю одного пикселя, при двух лучах — две
// (7.19б). Луч, попавший на ту же поверхность, берёт эти точные доли, попавший на
// другую — растянутые из картинки теней. Точные доли считаются, только если растянутые
// для точки пикселя ненадёжны (7.6с).

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
// Точные доли света пикселей группы: [пиксель][солнце, углы, лампа 1, лампа 2].
groupshared float parts[32][4];

// Какую долю света считает поток `lane` в группе ненадёжных пикселей (7.19б) при четырёх
// лучах: потоки 0–15 — первую лампу, 16–31 — вторую, 32–47 — углы, 48–63 — солнце.
static const uint PART_OF_QUARTER[4] = { 2, 3, 1, 0 };

[shader("compute")]
[numthreads(64, 1, 1)]
void cs_main(uint3 id : SV_DispatchThreadID, uint lane : SV_GroupIndex)
{
    // Лучей на пиксель: 4 или 2. Лучи пикселя — соседние потоки одной группы.
    bool pairs = (pc.flags & SPACE_FLAG_EDGE_PAIRS) != 0;
    uint shift = pairs ? 1 : 2;
    uint rays = 1u << shift;
    uint per = 64u >> shift;
    uint sub = id.x & (rays - 1);
    uint w, h;
    images[pc.target].GetDimensions(w, h);
    // Два списка (raymarch_edge_find.slang, 7.19а): сначала группы ненадёжных пикселей,
    // потом надёжных. Вид у всей группы один.
    uint count_u = pc.edges[4];
    uint count_r = pc.edges[5];
    uint groups_u = (count_u + per - 1) / per;
    uint group = id.x >> 6;
    bool unreliable = group < groups_u;
    uint first_j = (unreliable ? group : group - groups_u) * per;
    uint pixel = lane >> shift;
    uint j = first_j + pixel;
    bool valid = j < (unreliable ? count_u : count_r);
    int2 px = int2(0, 0);
    float4 g0 = float4(-1.0, 0.0, 0.0, 0.0);
    if (valid)
    {
        uint packed = pc.edges[8 + (unreliable ? j : w * h - 1 - j)];
        px = int2(packed & 0xffff, packed >> 16);
        g0 = gbuffer_load(px);
    }
    float3 n0 = oct_decode(g0.zw);

    // Точные доли света точки самого пикселя (солнце, углы, две лампы). Только там, где
    // тени из половинного размера для неё ненадёжны (7.6с): у силуэта большой формы её
    // сторона края и так покрыта похожими точками, и четыре луча тени на пиксель не
    // нужны. Надёжна ли точка, уже решил «поиск» (тот же вес, что набирает
    // upsampled_shadows_checked). Доли раздаются по долям, а не по пикселям (7.19б):
    // поток считает одну долю пикселя `lane % per` (при двух лучах — две по очереди), так
    // что 16 соседних потоков (при двух лучах — 32) идут одним и тем же лучом. В волне из
    // 32 потоков при четырёх лучах первая считает только лампы, вторая — углы и солнце
    // (в 2.5D-виде солнца нет, и там только углы); прежде в каждой волне были все четыре.
    float4 upsampled = float4(1.0, 1.0, 1.0, 1.0);
    if (unreliable)
    {
        uint mine = lane & (per - 1);
        bool has = first_j + mine < count_u;
        int2 at = int2(0, 0);
        float4 g = float4(-1.0, 0.0, 0.0, 0.0);
        if (has)
        {
            uint packed = pc.edges[8 + first_j + mine];
            at = int2(packed & 0xffff, packed >> 16);
            g = gbuffer_load(at);
        }
        float3 n = oct_decode(g.zw);
        float2 c = float2(at) + 0.5;
        Ray r0 = screen_ray(float2(c.x / float(w) * 2.0 - 1.0, 1.0 - c.y / float(h) * 2.0));
        float3 surface = r0.origin + r0.dir * g.x;
        uint calls = pairs ? 2 : 1;
        for (uint k = 0; k < calls; k++)
        {
            // Две доли при двух лучах: сначала лампы (потоки 0–31 — первая, 32–63 —
            // вторая), потом углы (0–31) и солнце (32–63).
            uint part = pairs ? (k == 0 ? 2 + (lane >> 5) : 1 - (lane >> 5))
                              : PART_OF_QUARTER[lane >> 4];
            float value = 1.0;
            if (g.x >= 0.0)
                value = shadow_part(surface, n, g.x, part);
            parts[mine][part] = value;
        }
    }
    else if (g0.x >= 0.0)
        upsampled = upsampled_shadows(px, g0.x, n0);
    counters_flush(PASS_EDGE_SHADOWS, id.x >> 6);
    GroupMemoryBarrierWithGroupSync();
    float4 exact = upsampled;
    if (unreliable)
        exact = float4(parts[pixel][0], parts[pixel][1], parts[pixel][2], parts[pixel][3]);

    float3 color = float3(0.0, 0.0, 0.0);
    if (valid)
    {
        float2 at = float2(px) + 0.5 + (pairs ? SUBPIXEL_PAIR[sub] : SUBPIXEL[sub]);
        float2 screen = float2(at.x / float(w) * 2.0 - 1.0, 1.0 - at.y / float(h) * 2.0);
        float2 middle = float2(px) + 0.5;
        float2 center = float2(middle.x / float(w) * 2.0 - 1.0, 1.0 - middle.y / float(h) * 2.0);
        // Полигоны (7.18е2): свой образец MSAA у каждого луча — край призмы ровный.
        float4 g = march_pixel(px, screen, int(sub), center);
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
    counters_flush(PASS_EDGES, id.x >> 6);
    // Складывает первый поток пикселя.
    colors[lane] = color;
    GroupMemoryBarrierWithGroupSync();
    if (valid && sub == 0)
    {
        float3 sum = colors[lane] + colors[lane + 1];
        if (!pairs)
            sum += colors[lane + 2] + colors[lane + 3];
        images[pc.target][px] = float4(sum / float(rays), 1.0);
    }
}

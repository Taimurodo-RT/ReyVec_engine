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

// Замер по частям (8.1, слово 6 заголовка списка, как EdgeProbe в demo.rs): проход
// пропускает одну часть работы, и по разнице времени на ПК видно, сколько она стоит.
// Картинка краёв при этом неверная.
static const uint EDGE_PROBE_NO_EXACT_SHADOWS = 1;
static const uint EDGE_PROBE_NO_SUB_RAYS = 2;
static const uint EDGE_PROBE_NO_SHADING = 3;
static const uint EDGE_PROBE_EMPTY = 4;
// Бит 8 того же слова (8.2): складки без доп. лучей. Слово 6 — замер (младшие 8 бит)
// и этот бит, как Demo::edge_list_word в demo.rs.
static const uint EDGE_CREASES = 0x100;

// Складка (8.2): пиксель, где сходятся две плоские (или почти плоские) поверхности —
// ребро коробки, стык передней грани и бока выдавленной фигуры, угол пола и стены. Тут
// доп. лучам незачем шагать по сцене: каждый луч пересекает обе плоскости, и видна та, в
// которую он входит в тело (у выпуклого ребра — дальняя, у вогнутого угла — ближняя).
// Плоскости — точка самого пикселя и точка соседа на другой стороне ребра. Пиксель
// считается складкой, только если эта модель повторяет буфер геометрии: середина пикселя
// попадает в свою плоскость, середина соседа — в его, а остальные соседи лежат на одной
// из двух. Иначе (небо рядом, разрыв по глубине, третья поверхность, образцы полигонов
// расходятся) — обычные доп. лучи.
struct Crease
{
    bool ok;
    float3 p0;
    float3 n0;
    float4 g1; // буфер геометрии соседа за ребром
    float3 p1;
    float3 n1;
    bool convex;
};

// Расстояние вдоль луча до плоскости (точка `p`, нормаль `n`); −1 — луч почти вдоль неё.
float plane_t(Ray r, float3 p, float3 n)
{
    float facing = dot(r.dir, n);
    return abs(facing) > 0.05 ? dot(p - r.origin, n) / facing : -1.0;
}

// Что видно на луче `r` по модели складки: x — расстояние (−1 — модель не годится),
// y — 0 своя плоскость, 1 — соседа.
float2 crease_hit(Crease c, Ray r)
{
    float ta = plane_t(r, c.p0, c.n0);
    float tb = plane_t(r, c.p1, c.n1);
    if (ta <= 0.0 || tb <= 0.0)
        return float2(-1.0, 0.0);
    bool other = c.convex ? tb > ta : tb < ta;
    return float2(other ? tb : ta, other ? 1.0 : 0.0);
}

float2 screen_of(float2 at, uint w, uint h)
{
    return float2(at.x / float(w) * 2.0 - 1.0, 1.0 - at.y / float(h) * 2.0);
}

bool near_t(float a, float b)
{
    return abs(a - b) <= 0.01 * b + 0.01;
}

Crease find_crease(int2 px, float4 g0, uint w, uint h)
{
    Crease c;
    c.ok = false;
    c.g1 = float4(-1.0, 0.0, 0.0, 0.0);
    c.p0 = float3(0.0, 0.0, 0.0);
    c.n0 = oct_decode(g0.zw);
    c.p1 = c.p0;
    c.n1 = c.n0;
    c.convex = true;
    if (g0.x < 0.0 || mesh_mixed(px, g0.x))
        return c;
    Ray r0 = screen_ray(screen_of(float2(px) + 0.5, w, h));
    c.p0 = r0.origin + r0.dir * g0.x;
    bool have = false;
    for (uint k = 0; k < 4; k++)
    {
        int2 o = k < 2 ? int2(int(k) * 2 - 1, 0) : int2(0, int(k) * 2 - 5);
        int2 q = px + o;
        if (any(q < 0) || q.x >= int(w) || q.y >= int(h))
            continue;
        float4 g1 = gbuffer_load(q);
        if (g1.x < 0.0)
            return c;
        Ray r1 = screen_ray(screen_of(float2(q) + 0.5, w, h));
        float3 n1 = oct_decode(g1.zw);
        // Сосед на своей плоскости: тот же материал, та же сторона, попадание по плоскости.
        float ta = plane_t(r1, c.p0, c.n0);
        if (abs(g1.y - g0.y) < 0.5 && dot(n1, c.n0) > 0.9 && ta > 0.0 && near_t(g1.x, ta))
            continue;
        float3 p1 = r1.origin + r1.dir * g1.x;
        if (have)
        {
            // Второй сосед за ребром — на той же второй плоскости, иначе это не складка.
            float tb = plane_t(r1, c.p1, c.n1);
            if (abs(g1.y - c.g1.y) < 0.5 && dot(n1, c.n1) > 0.9 && tb > 0.0 && near_t(g1.x, tb))
                continue;
            return c;
        }
        have = true;
        c.g1 = g1;
        c.p1 = p1;
        c.n1 = n1;
    }
    // Плоскости почти параллельны — это ступенька по глубине, а не ребро.
    if (!have || abs(dot(c.n0, c.n1)) > 0.995)
        return c;
    c.convex = dot(c.p1 - c.p0, c.n0) < 0.0;
    // Модель повторяет буфер геометрии в середине пикселя и у всех соседей.
    float2 mine = crease_hit(c, r0);
    if (mine.x < 0.0 || mine.y != 0.0 || !near_t(mine.x, g0.x))
        return c;
    for (uint k = 0; k < 4; k++)
    {
        int2 o = k < 2 ? int2(int(k) * 2 - 1, 0) : int2(0, int(k) * 2 - 5);
        int2 q = px + o;
        if (any(q < 0) || q.x >= int(w) || q.y >= int(h))
            continue;
        float4 g1 = gbuffer_load(q);
        float2 hit = crease_hit(c, screen_ray(screen_of(float2(q) + 0.5, w, h)));
        if (hit.x < 0.0 || !near_t(hit.x, g1.x))
            return c;
    }
    c.ok = true;
    return c;
}

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
    uint probe = pc.edges[6] & 0xff;
    bool creases = (pc.edges[6] & EDGE_CREASES) != 0;
    if (probe == EDGE_PROBE_EMPTY)
        return;
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
            if (g.x >= 0.0 && probe != EDGE_PROBE_NO_EXACT_SHADOWS)
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
        // Складка (8.2): доп. луч берёт одну из двух плоскостей без шагов по сцене.
        // Полигоны (7.18е2): свой образец MSAA у каждого луча — край призмы ровный.
        float4 g = g0;
        bool marched = probe != EDGE_PROBE_NO_SUB_RAYS;
        if (marched && creases)
        {
            Crease c = find_crease(px, g0, w, h);
            if (c.ok)
            {
                float2 hit = crease_hit(c, screen_ray(screen));
                if (hit.x >= 0.0)
                {
                    g = hit.y == 0.0 ? float4(hit.x, g0.yzw) : float4(hit.x, c.g1.yzw);
                    marched = false;
                }
            }
        }
        if (marched)
            g = march_pixel(px, screen, int(sub), center);
        if (g.x < 0.0)
            color = sky(screen_ray(screen).dir);
        else
        {
            float3 n = oct_decode(g.zw);
            if (probe == EDGE_PROBE_NO_SHADING)
                color = n * 0.5 + 0.5;
            else
            {
                // Та же поверхность, что у точки пикселя: тот же материал, глубина и
                // сторона.
                bool same = g0.x >= 0.0 && abs(g.y - g0.y) < 0.5
                    && abs(g.x - g0.x) < 0.02 * g0.x + 0.05 && dot(n, n0) > 0.9;
                float4 shadows = same ? exact : upsampled_shadows(px, g.x, n);
                color = shade_lit(screen, g, shadows);
            }
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

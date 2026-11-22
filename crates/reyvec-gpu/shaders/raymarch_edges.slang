// Сцена «3D», проход «края 3D» (7.6о): сглаживание краёв форм. Для каждого пикселя из
// списка (raymarch_edge_find.slang) четыре потока пускают по лучу через разные точки
// пикселя (повёрнутая решётка 4×: у всех лучей разные x и y, поэтому и вертикальные, и
// горизонтальные края получают четыре ступени), шагают и освещают их как обычно, а
// среднее пишут в картинку краёв. «Сцена» берёт его вместо цвета одного луча.
//
// Конус квадрата 8×8 (7.8) накрывает весь квадрат, поэтому любой луч внутри пикселя
// может начинать с его длины; полигоны берутся из самого пикселя.
//
// Тени на краях: у ненадёжных пикселей (7.6с) точные доли света своей точки пикселя
// уже посчитал проход «края 3D: тени» (raymarch_edge_shadows.slang, 8.22) и оставил в
// картинке краёв; у надёжных они растянуты из картинки теней. Луч, попавший на ту же
// поверхность, берёт эти доли, попавший на другую — растянутые из картинки теней.

[[vk::binding(2, 0)]] [format("rgba16f")] RWTexture2D<float4> images[];

#include "include/march.slang"
#include "include/shade3d.slang"
#include "include/shadow3d.slang"
#include "include/edges.slang"

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

// Откуда доп. лучу пикселя `px` начинать шаги (8.23): чуть ближе ближайшей точки
// буфера геометрии в квадрате 3×3 вокруг. Все девять лучей середин дошли до своих точек
// по пустоте, а доп. луч идёт между ними, поэтому и у него до этой глубины пусто (запас
// 2 % и 2 см — на разницу направлений внутри пикселя). На заводе так отпадает шестая
// часть шагов доп. лучей, и ни один луч не попадает ближе начала (счётчики в CI). Небо
// вокруг не в счёт; если вокруг одно небо — с начала конуса.
float near_start(int2 px, uint w, uint h)
{
    float nearest = 1e9;
    for (int dy = -1; dy <= 1; dy++)
    {
        for (int dx = -1; dx <= 1; dx++)
        {
            int2 q = clamp(px + int2(dx, dy), int2(0, 0), int2(int(w), int(h)) - 1);
            float d = gbuffer_load(q).x;
            if (d >= 0.0)
                nearest = min(nearest, d);
        }
    }
    return nearest < 1e8 ? max(nearest * 0.98 - 0.02, 0.0) : 0.0;
}

groupshared float3 colors[64];

[shader("compute")]
[numthreads(64, 1, 1)]
void cs_main(uint3 id : SV_DispatchThreadID, uint lane : SV_GroupIndex)
{
    // Лучей на пиксель: 4 или 2. Лучи пикселя — соседние потоки одной группы.
    uint probe = pc.edges[6] & 0xff;
    bool creases = (pc.edges[6] & EDGE_CREASES) != 0;
    if (probe == EDGE_PROBE_EMPTY)
        return;
    uint w, h;
    images[pc.target].GetDimensions(w, h);
    // Четыре части списка (include/edges.slang, 8.11б): группы идут в порядке EDGE_ORDER,
    // у каждой группы одна часть — ненадёжные или надёжные, четыре или два луча.
    uint group = id.x >> 6;
    uint list = EDGE_ORDER[EDGE_LISTS - 1];
    uint first_j = 0;
    uint count = 0;
    {
        uint before = 0;
        for (uint o = 0; o < EDGE_LISTS; o++)
        {
            uint k = EDGE_ORDER[o];
            uint n = pc.edges[EDGE_COUNT_WORD[k]];
            uint per_k = edge_list_per(k);
            uint groups_k = (n + per_k - 1) / per_k;
            if (group < before + groups_k || o == EDGE_LISTS - 1)
            {
                list = k;
                first_j = (group - before) * per_k;
                count = n;
                break;
            }
            before += groups_k;
        }
    }
    bool pairs = list >= EDGE_U2;
    uint shift = pairs ? 1 : 2;
    uint rays = 1u << shift;
    uint per = 64u >> shift;
    uint sub = id.x & (rays - 1);
    bool unreliable = list == EDGE_U4 || list == EDGE_U2;
    uint pixel = lane >> shift;
    uint j = first_j + pixel;
    bool valid = j < count;
    int2 px = int2(0, 0);
    float4 g0 = float4(-1.0, 0.0, 0.0, 0.0);
    if (valid)
    {
        uint packed = pc.edges[edge_list_at(list, j, w * h)];
        px = int2(packed & 0xffff, packed >> 16);
        g0 = gbuffer_load(px);
    }
    float3 n0 = oct_decode(g0.zw);

    // Точные доли света точки самого пикселя (солнце, углы, две лампы) у ненадёжных
    // пикселей уже посчитал проход «края 3D: тени» (raymarch_edge_shadows.slang, 8.22) и
    // оставил в картинке краёв на месте пикселя; цвет ниже их заменит. Надёжным хватает
    // теней из половинного размера.
    float4 exact = float4(1.0, 1.0, 1.0, 1.0);
    if (unreliable)
    {
        if (valid && probe != EDGE_PROBE_NO_EXACT_SHADOWS)
            exact = images[pc.target][px];
    }
    else if (g0.x >= 0.0)
        exact = upsampled_shadows(px, g0.x, n0);

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
            g = march_pixel(px, screen, int(sub), center, near_start(px, w, h));
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
                color = shade_lit(screen, g, shadows, px, int(sub));
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

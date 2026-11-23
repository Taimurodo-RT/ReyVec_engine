// Сцена «3D», проход «края 3D: поиск» (7.6о): находит пиксели на краях форм, где один
// луч на пиксель даёт лесенку. Край — там, где у соседа (справа, слева, сверху, снизу)
// другая поверхность: небо против формы, другой материал, резкий поворот нормали или
// разрыв по глубине (сосед не лежит на плоскости этой точки), а у полигонов с образцами
// MSAA ещё и пиксель, где образцы расходятся (mesh_mixed). Такие пиксели попадают в
// список `pc.edges`, проход «края 3D» (raymarch_edges.slang) пускает в них по четыре
// луча внутри пикселя. Остальным пикселям картинки краёв ставится прозрачность 0:
// «сцена» по ней узнаёт, где брать готовый цвет края.
//
// Пиксели края делятся на два списка (7.19а): «ненадёжные» — тени из половинного размера
// для точки пикселя к ней не подходят (shadow_similarity < 0.5 в shade3d.slang: тот же вес,
// что набирает upsampled_shadows_checked), им «края 3D» пускают свои лучи тени; остальные
// «надёжные». Так в группе прохода «края
// 3D» пиксели только одного вида, и лучи тени не ждут в ней пикселей, которым они не нужны.
//
// Список (`uint`, раскладка — include/edges.slang): заголовок из EDGE_HEADER слов, потом
// четыре части списка (8.11б): ненадёжные и надёжные, с четырьмя и двумя лучами. Начало
// заголовка пишет процессор перед проходом.

#include "include/space.slang"
#include "include/mesh.slang"
#include "include/shade3d.slang"
#include "include/edges.slang"

[[vk::binding(2, 0)]] [format("rgba16f")] RWTexture2D<float4> images[];

// Точка экрана центра пикселя `px` кадра размером `size`.
float2 pixel_screen(int2 px, float2 size)
{
    float2 c = float2(px) + 0.5;
    return float2(c.x / size.x * 2.0 - 1.0, 1.0 - c.y / size.y * 2.0);
}

// MAX_EDGES — в include/edges.slang.

// Почему пиксель — край (8.11а, счётчики краёв по причине): 0 — не край.
static const uint EDGE_NONE = 0;
static const uint EDGE_MESH = 1;     // образцы полигонов в пикселе расходятся
static const uint EDGE_SKY = 2;      // небо против формы
static const uint EDGE_MATERIAL = 3; // другой материал
static const uint EDGE_NORMAL = 4;   // резкий поворот нормали (складка или силуэт)
static const uint EDGE_DEPTH = 5;    // разрыв по глубине (силуэт на фоне той же стороны)

// Причина края у пикселя `px` с буфером геометрии `g0` и сосед `q`, у которого она
// найдена (для EDGE_MESH и EDGE_NONE — сам `px`).
uint edge_reason(int2 px, int2 size, float4 g0, out int2 q)
{
    q = px;
    // Полигоны с образцами MSAA (7.18е2): пиксель, где образцы расходятся, — край, даже
    // если соседи по буферу геометрии одинаковы (щель уже пикселя).
    if (mesh_mixed(px, g0.x))
        return EDGE_MESH;
    Ray r0 = screen_ray(pixel_screen(px, float2(size)));
    float3 n0 = oct_decode(g0.zw);
    float3 p0 = r0.origin + r0.dir * g0.x;
    for (uint k = 0; k < 4; k++)
    {
        int2 o = k < 2 ? int2(int(k) * 2 - 1, 0) : int2(0, int(k) * 2 - 5);
        q = px + o;
        if (any(q < 0) || any(q >= size))
            continue;
        float4 g1 = gbuffer_load(q);
        if ((g0.x < 0.0) != (g1.x < 0.0))
            return EDGE_SKY;
        if (g0.x < 0.0)
            continue;
        if (abs(g0.y - g1.y) > 0.5)
            return EDGE_MATERIAL;
        if (dot(n0, oct_decode(g1.zw)) < 0.9)
            return EDGE_NORMAL;
        // Где сосед был бы, лежи он на плоскости этой точки: на ровном полу и гранях
        // (даже под острым углом к лучу) совпадает, на крае формы — нет.
        Ray r1 = screen_ray(pixel_screen(q, float2(size)));
        float facing = dot(r1.dir, n0);
        float expected = abs(facing) > 1e-4 ? dot(p0 - r1.origin, n0) / facing : 1e30;
        if (abs(g1.x - expected) > 0.01 * g0.x + 0.01)
            return EDGE_DEPTH;
    }
    q = px;
    return EDGE_NONE;
}

#ifdef SPACE_COUNTERS
// Счётчики краёв (8.11а): у прохода PASS_EDGE_FIND свои значения в тех же восьми словах,
// что у лучей (раскладка — EdgeCounts в crates/reyvec-gpu/src/counters.rs): пикселей края,
// по причине (образцы полигонов, небо, материал, нормаль, глубина) и сколько из них
// почти не видно глазом: цвет пикселя и соседа за краем после перевода в 8 бит экрана
// различается не больше чем на 2 и на 8 ступеней. Цвет — как у «сцены» без сглаживания
// (тени из картинки половинного размера, текстура полигонов не учитывается), перевод в
// экран — степенью 1/2.2 без мягкого перехода к белому: для отбора идей, не для картинки.
void count_edge(int2 px, int2 size, float4 g0, uint reason, int2 q)
{
    uint differs = 255;
    if (reason != EDGE_NONE && reason != EDGE_MESH)
    {
        float3 a = shade_point(pixel_screen(px, float2(size)), g0, px);
        float3 b = shade_point(pixel_screen(q, float2(size)), gbuffer_load(q), q);
        float3 d = abs(pow(saturate(a), 1.0 / 2.2) - pow(saturate(b), 1.0 / 2.2)) * 255.0;
        differs = uint(ceil(max(d.r, max(d.g, d.b))));
    }
    // Вычисления сцены внутри освещения не считаются: только числа краёв.
    counters_reset();
    bool edge = reason != EDGE_NONE;
    counted_rays = edge ? 1 : 0;
    counted_steps = reason == EDGE_MESH ? 1 : 0;
    counted_scene = reason == EDGE_SKY ? 1 : 0;
    counted_normal = reason == EDGE_MATERIAL ? 1 : 0;
    counted_objects = reason == EDGE_NORMAL ? 1 : 0;
    counted_forms = reason == EDGE_DEPTH ? 1 : 0;
    counted_field = edge && differs <= 2 ? 1 : 0;
    counted_material = edge && differs <= 8 ? 1 : 0;
}
#endif

// `g0` — буфер геометрии в `px`.
bool is_edge(int2 px, int2 size, float4 g0)
{
    int2 q;
    return edge_reason(px, size, g0, q) != EDGE_NONE;
}

// Файл обязан иметь пару для рисования (build.rs собирает её у всех шейдеров); здесь
// она не используется.
[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    return float4(0.0, 0.0, 0.0, 0.0);
}

// Края группы 8×8: сначала собираются здесь, потом группа одним сложением берёт место в
// общем списке (7.15). Сложение в общий счётчик от каждого пикселя края по отдельности
// на встроенной видеокарте стоило почти миллисекунду. Четыре части списка (8.11б):
// ненадёжные и надёжные, каждые с четырьмя или двумя лучами.
groupshared uint group_count[EDGE_LISTS];
groupshared uint group_base[EDGE_LISTS];
groupshared uint group_keep[EDGE_LISTS];
groupshared uint group_pixels[EDGE_LISTS][64];

// Сколько новых границ групп прохода «края 3D» (по `per` пикселей; номера 0, per, 2·per…)
// переходит часть списка с `base` длиной `count`.
uint groups_crossed(uint base, uint count, uint per)
{
    return (base + count + per - 1) / per - (base + per - 1) / per;
}

// Сколько метров сцены накрывает пиксель `px` на глубине `t` (8.11б): расстояние между
// точками соседних лучей на этой глубине.
float pixel_footprint(int2 px, int2 size, float t)
{
    Ray a = screen_ray(pixel_screen(px, float2(size)));
    Ray b = screen_ray(pixel_screen(px + int2(1, 0), float2(size)));
    return length((b.origin + b.dir * t) - (a.origin + a.dir * t));
}

[shader("compute")]
[numthreads(8, 8, 1)]
void cs_main(uint3 id : SV_DispatchThreadID, uint lane : SV_GroupIndex)
{
    if (lane < EDGE_LISTS)
    {
        group_count[lane] = 0;
        group_keep[lane] = 0;
    }
    GroupMemoryBarrierWithGroupSync();
    int2 size = gbuffer_size();
    uint w = uint(size.x), h = uint(size.y);
    int2 px = int2(id.xy);
    bool inside = id.x < w && id.y < h;
    float4 g0 = inside ? gbuffer_load(px) : float4(-1.0, 0.0, 0.0, 0.0);
    int2 across;
    uint reason = inside ? edge_reason(px, int2(w, h), g0, across) : EDGE_NONE;
    bool edge = reason != EDGE_NONE;
#ifdef SPACE_COUNTERS
    if (inside)
        count_edge(px, int2(w, h), g0, reason, across);
    counters_flush(PASS_EDGE_FIND, counter_spread(px));
#endif
    if (edge)
    {
        // Картинка теней — половина кадра с округлением вверх (как в demo.rs).
        bool unreliable = g0.x >= 0.0
            && shadow_similarity(px, g0.x, oct_decode(g0.zw), (size + 1) >> 1) < 0.5;
        // Два луча вместо четырёх (8.11б): при уменьшенном разрешении — у всех, а с
        // включённым «края: два луча вдали» — там, где пиксель накрывает больше порога
        // (дальние края; у неба порога нет).
        float far = asfloat(pc.edges[EDGE_FAR_WORD]);
        bool pair = (pc.flags & SPACE_FLAG_EDGE_PAIRS) != 0
            || (far > 0.0 && g0.x >= 0.0 && pixel_footprint(px, int2(w, h), g0.x) > far);
        uint list = (unreliable ? EDGE_U4 : EDGE_R4) + (pair ? 2 : 0);
        uint slot;
        InterlockedAdd(group_count[list], 1u, slot);
        group_pixels[list][slot] = id.x | (id.y << 16);
    }
    GroupMemoryBarrierWithGroupSync();
    if (lane == 0)
    {
        uint all = 0;
        for (uint k = 0; k < EDGE_LISTS; k++)
            all += group_count[k];
        if (all > 0)
        {
            // Место в общем счёте: что за MAX_EDGES, остаётся с одним лучом (внутри группы
            // сначала отбрасываются надёжные; какие группы опоздали — как придётся).
            uint total;
            InterlockedAdd(pc.edges[3], all, total);
            uint room = total < MAX_EDGES ? MAX_EDGES - total : 0;
            uint groups = 0;
            uint shadow_groups = 0;
            // Порядок: ненадёжные (4 и 2 луча), потом надёжные.
            for (uint o = 0; o < EDGE_LISTS; o++)
            {
                uint k = EDGE_ORDER[o];
                uint keep = min(group_count[k], room);
                room -= keep;
                group_keep[k] = keep;
                if (keep > 0)
                {
                    uint base;
                    InterlockedAdd(pc.edges[EDGE_COUNT_WORD[k]], keep, base);
                    group_base[k] = base;
                    uint crossed = groups_crossed(base, keep, edge_list_per(k));
                    groups += crossed;
                    if (k == EDGE_U4 || k == EDGE_U2)
                        shadow_groups += groups_crossed(base, keep, EDGE_SHADOW_PIXELS);
                }
            }
            if (groups > 0)
                InterlockedAdd(pc.edges[0], groups);
            // «Края 3D: тени» (8.24): по EDGE_SHADOW_PIXELS ненадёжных на группу.
            if (shadow_groups > 0)
                InterlockedAdd(pc.edges[EDGE_SHADOW_GROUPS_WORD], shadow_groups);
        }
    }
    GroupMemoryBarrierWithGroupSync();
    // Края группы — в списки по местам от начала группы (пишет поток с тем же номером,
    // какой бы пиксель он ни смотрел).
    for (uint k = 0; k < EDGE_LISTS; k++)
    {
        if (lane < group_keep[k])
            pc.edges[edge_list_at(k, group_base[k] + lane, w * h)] = group_pixels[k][lane];
    }
    if (inside)
        images[pc.target][px] = float4(0.0, 0.0, 0.0, 0.0);
}

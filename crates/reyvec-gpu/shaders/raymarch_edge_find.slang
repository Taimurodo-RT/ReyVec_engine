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
// Список (`uint`): [0] — сколько групп по 64 потока запустить (16 пикселей на группу,
// 32 при SPACE_FLAG_EDGE_PAIRS; сначала группы ненадёжных, потом надёжных),
// [1], [2] — 1 (размеры запуска), [3] — сколько пикселей всего (его читает панель),
// [4] — сколько ненадёжных, [5] — сколько надёжных (только вошедших в список, вместе
// не больше MAX_EDGES), [6], [7] — не заняты. Дальше место
// на w·h пикселей (x | y << 16): ненадёжные от начала (`pc.edges[8 + j]`), надёжные от
// конца (`pc.edges[8 + w·h − 1 − j]`). Начало [0, 1, 1, 0, 0, 0, 0, 0] пишет процессор
// перед проходом.

#include "include/space.slang"
#include "include/mesh.slang"
#include "include/shade3d.slang"

[[vk::binding(2, 0)]] [format("rgba16f")] RWTexture2D<float4> images[];

// Точка экрана центра пикселя `px` кадра размером `size`.
float2 pixel_screen(int2 px, float2 size)
{
    float2 c = float2(px) + 0.5;
    return float2(c.x / size.x * 2.0 - 1.0, 1.0 - c.y / size.y * 2.0);
}

// Больше групп одной командой видеокарта может не запустить (гарантировано 65535). Два
// списка дают до ⌈U/16⌉ + ⌈R/16⌉ групп: на одну больше, чем один, поэтому 65534.
static const uint MAX_EDGES = 65534 * 16;

// `g0` — буфер геометрии в `px`.
bool is_edge(int2 px, int2 size, float4 g0)
{
    // Полигоны с образцами MSAA (7.18е2): пиксель, где образцы расходятся, — край, даже
    // если соседи по буферу геометрии одинаковы (щель уже пикселя).
    if (mesh_mixed(px, g0.x))
        return true;
    Ray r0 = screen_ray(pixel_screen(px, float2(size)));
    float3 n0 = oct_decode(g0.zw);
    float3 p0 = r0.origin + r0.dir * g0.x;
    for (uint k = 0; k < 4; k++)
    {
        int2 o = k < 2 ? int2(int(k) * 2 - 1, 0) : int2(0, int(k) * 2 - 5);
        int2 q = px + o;
        if (any(q < 0) || any(q >= size))
            continue;
        float4 g1 = gbuffer_load(q);
        if ((g0.x < 0.0) != (g1.x < 0.0))
            return true;
        if (g0.x < 0.0)
            continue;
        if (abs(g0.y - g1.y) > 0.5)
            return true;
        if (dot(n0, oct_decode(g1.zw)) < 0.9)
            return true;
        // Где сосед был бы, лежи он на плоскости этой точки: на ровном полу и гранях
        // (даже под острым углом к лучу) совпадает, на крае формы — нет.
        Ray r1 = screen_ray(pixel_screen(q, float2(size)));
        float facing = dot(r1.dir, n0);
        float expected = abs(facing) > 1e-4 ? dot(p0 - r1.origin, n0) / facing : 1e30;
        if (abs(g1.x - expected) > 0.01 * g0.x + 0.01)
            return true;
    }
    return false;
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
// на встроенной видеокарте стоило почти миллисекунду.
groupshared uint group_count[2];
groupshared uint group_base[2];
groupshared uint group_keep[2];
groupshared uint group_pixels[2][64];

// Сколько новых границ групп прохода «края 3D» (по `per` пикселей; номера 0, per, 2·per…)
// переходит часть списка с `base` длиной `count`.
uint groups_crossed(uint base, uint count, uint per)
{
    return (base + count + per - 1) / per - (base + per - 1) / per;
}

[shader("compute")]
[numthreads(8, 8, 1)]
void cs_main(uint3 id : SV_DispatchThreadID, uint lane : SV_GroupIndex)
{
    if (lane < 2)
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
    bool edge = inside && is_edge(px, int2(w, h), g0);
    if (edge)
    {
        // Картинка теней — половина кадра с округлением вверх (как в demo.rs).
        bool unreliable = g0.x >= 0.0
            && shadow_similarity(px, g0.x, oct_decode(g0.zw), (size + 1) >> 1) < 0.5;
        uint list = unreliable ? 0 : 1;
        uint slot;
        InterlockedAdd(group_count[list], 1u, slot);
        group_pixels[list][slot] = id.x | (id.y << 16);
    }
    GroupMemoryBarrierWithGroupSync();
    uint count_u = group_count[0];
    uint count_r = group_count[1];
    if (lane == 0 && count_u + count_r > 0)
    {
        // Место в общем счёте: что за MAX_EDGES, остаётся с одним лучом (внутри группы
        // сначала отбрасываются надёжные; какие группы опоздали — как придётся).
        uint total;
        InterlockedAdd(pc.edges[3], count_u + count_r, total);
        uint room = total < MAX_EDGES ? MAX_EDGES - total : 0;
        uint keep_u = min(count_u, room);
        uint keep_r = min(count_r, room - keep_u);
        group_keep[0] = keep_u;
        group_keep[1] = keep_r;
        uint per = (pc.flags & SPACE_FLAG_EDGE_PAIRS) != 0 ? 32 : 16;
        uint groups = 0;
        if (keep_u > 0)
        {
            uint base;
            InterlockedAdd(pc.edges[4], keep_u, base);
            group_base[0] = base;
            groups += groups_crossed(base, keep_u, per);
        }
        if (keep_r > 0)
        {
            uint base;
            InterlockedAdd(pc.edges[5], keep_r, base);
            group_base[1] = base;
            groups += groups_crossed(base, keep_r, per);
        }
        if (groups > 0)
            InterlockedAdd(pc.edges[0], groups);
    }
    GroupMemoryBarrierWithGroupSync();
    // Края группы — в списки по местам от начала группы (пишет поток с тем же номером,
    // какой бы пиксель он ни смотрел).
    if (lane < group_keep[0])
        pc.edges[8 + group_base[0] + lane] = group_pixels[0][lane];
    if (lane < group_keep[1])
        pc.edges[8 + w * h - 1 - (group_base[1] + lane)] = group_pixels[1][lane];
    if (inside)
        images[pc.target][px] = float4(0.0, 0.0, 0.0, 0.0);
}

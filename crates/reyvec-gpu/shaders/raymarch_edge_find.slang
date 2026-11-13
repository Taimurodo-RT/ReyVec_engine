// Сцена «3D», проход «края 3D: поиск» (7.6о): находит пиксели на краях форм, где один
// луч на пиксель даёт лесенку. Край — там, где у соседа (справа, слева, сверху, снизу)
// другая поверхность: небо против формы, другой материал, резкий поворот нормали или
// разрыв по глубине (сосед не лежит на плоскости этой точки), а у полигонов с образцами
// MSAA ещё и пиксель, где образцы расходятся (mesh_mixed). Такие пиксели попадают в
// список `pc.edges`, проход «края 3D» (raymarch_edges.slang) пускает в них по четыре
// луча внутри пикселя. Остальным пикселям картинки краёв ставится прозрачность 0:
// «сцена» по ней узнаёт, где брать готовый цвет края.
//
// Список (`uint`): [0] — сколько групп по 64 потока запустить (16 пикселей на группу,
// 32 при SPACE_FLAG_EDGE_PAIRS),
// [1], [2] — 1 (размеры запуска), [3] — сколько пикселей, дальше пиксели (x | y << 16).
// Начало [0, 1, 1, 0] пишет процессор перед проходом.

#include "include/space.slang"
#include "include/mesh.slang"

[[vk::binding(2, 0)]] [format("rgba16f")] RWTexture2D<float4> images[];

// Точка экрана центра пикселя `px` кадра размером `size`.
float2 pixel_screen(int2 px, float2 size)
{
    float2 c = float2(px) + 0.5;
    return float2(c.x / size.x * 2.0 - 1.0, 1.0 - c.y / size.y * 2.0);
}

// Больше групп одной командой видеокарта может не запустить (гарантировано 65535).
static const uint MAX_EDGES = 65535 * 16;

bool is_edge(int2 px, int2 size)
{
    float4 g0 = gbuffer_load(px);
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
groupshared uint group_count;
groupshared uint group_base;
groupshared uint group_pixels[64];

[shader("compute")]
[numthreads(8, 8, 1)]
void cs_main(uint3 id : SV_DispatchThreadID, uint lane : SV_GroupIndex)
{
    if (lane == 0)
        group_count = 0;
    GroupMemoryBarrierWithGroupSync();
    int2 size = gbuffer_size();
    uint w = uint(size.x), h = uint(size.y);
    int2 px = int2(id.xy);
    bool inside = id.x < w && id.y < h;
    bool edge = inside && is_edge(px, int2(w, h));
    if (edge)
    {
        uint slot;
        InterlockedAdd(group_count, 1u, slot);
        group_pixels[slot] = id.x | (id.y << 16);
    }
    GroupMemoryBarrierWithGroupSync();
    uint count = group_count;
    if (lane == 0 && count > 0)
    {
        uint base;
        InterlockedAdd(pc.edges[3], count, base);
        group_base = base;
        // Группы прохода «края 3D» по 16 пикселей (по 32 при двух лучах): сколько новых
        // границ групп перешёл список (номера 0, 16, 32… — как раньше по одной на пиксель).
        uint per = (pc.flags & SPACE_FLAG_EDGE_PAIRS) != 0 ? 32 : 16;
        uint end = min(base + count, MAX_EDGES);
        uint groups = base < MAX_EDGES ? (end + per - 1) / per - (base + per - 1) / per : 0;
        if (groups > 0)
            InterlockedAdd(pc.edges[0], groups);
    }
    GroupMemoryBarrierWithGroupSync();
    // Края группы — в список по местам от начала группы (пишет поток с тем же номером,
    // какой бы пиксель он ни смотрел). Не влезшие остаются с одним лучом.
    if (lane < count)
    {
        uint index = group_base + lane;
        if (index < MAX_EDGES)
            pc.edges[4 + index] = group_pixels[lane];
    }
    if (inside)
        images[pc.target][px] = float4(0.0, 0.0, 0.0, 0.0);
}

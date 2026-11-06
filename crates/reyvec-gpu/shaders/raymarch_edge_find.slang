// Сцена «3D», проход «края 3D: поиск» (7.6о): находит пиксели на краях форм, где один
// луч на пиксель даёт лесенку. Край — там, где у соседа (справа, слева, сверху, снизу)
// другая поверхность: небо против формы, другой материал, резкий поворот нормали или
// разрыв по глубине (сосед не лежит на плоскости этой точки). Такие пиксели попадают в
// список `pc.edges`, проход «края 3D» (raymarch_edges.slang) пускает в них по четыре
// луча внутри пикселя. Остальным пикселям картинки краёв ставится прозрачность 0:
// «сцена» по ней узнаёт, где брать готовый цвет края.
//
// Список (`uint`): [0] — сколько групп по 64 потока запустить (16 пикселей на группу),
// [1], [2] — 1 (размеры запуска), [3] — сколько пикселей, дальше пиксели (x | y << 16).
// Начало [0, 1, 1, 0] пишет процессор перед проходом.

#include "include/space.slang"

[[vk::binding(0, 0)]] Texture2D textures[];
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
    float4 g0 = textures[pc.gbuffer].Load(int3(px, 0));
    Ray r0 = screen_ray(pixel_screen(px, float2(size)));
    float3 n0 = oct_decode(g0.zw);
    float3 p0 = r0.origin + r0.dir * g0.x;
    for (uint k = 0; k < 4; k++)
    {
        int2 o = k < 2 ? int2(int(k) * 2 - 1, 0) : int2(0, int(k) * 2 - 5);
        int2 q = px + o;
        if (any(q < 0) || any(q >= size))
            continue;
        float4 g1 = textures[pc.gbuffer].Load(int3(q, 0));
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

[shader("compute")]
[numthreads(8, 8, 1)]
void cs_main(uint3 id : SV_DispatchThreadID)
{
    uint w, h;
    textures[pc.gbuffer].GetDimensions(w, h);
    if (id.x >= w || id.y >= h)
        return;
    int2 px = int2(id.xy);
    if (!is_edge(px, int2(w, h)))
    {
        images[pc.target][px] = float4(0.0, 0.0, 0.0, 0.0);
        return;
    }
    uint index;
    InterlockedAdd(pc.edges[3], 1u, index);
    if (index >= MAX_EDGES)
    {
        // Краёв больше, чем можно запустить одной командой: этот остаётся с одним лучом.
        images[pc.target][px] = float4(0.0, 0.0, 0.0, 0.0);
        return;
    }
    pc.edges[4 + index] = id.x | (id.y << 16);
    // Каждые 16 пикселей — ещё одна группа прохода «края 3D».
    if ((index & 15) == 0)
    {
        uint groups;
        InterlockedAdd(pc.edges[0], 1u, groups);
    }
}

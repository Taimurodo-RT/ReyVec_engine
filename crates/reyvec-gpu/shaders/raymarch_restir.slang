// Сцена «3D», проход «ReSTIR: кандидаты» (путь ReSTIR, шаг 3, include/restir.slang): у
// каждого пикселя режима «качество» — свой запас точки света. Пиксель пробует
// RESTIR_CANDIDATES случайных точек ламп (без лучей тени: только «сколько света
// дала бы»), потом берёт запас своего места в прошлом кадре. Итог — в слой 1 буфера
// запасов; соседей и луч тени берёт проход «путь 3D».
//
// Константы: pc.gbuffer — буфер геометрии, pc.edges — буфер запасов, pc.meshes — номер
// кадра, pc.target — картинка того же размера (только чтобы узнать размер).
//
// С битом RESTIR_GI_NEIGHBORS в pc.flags — проход «ReSTIR GI: соседи» (шаг 4,
// include/restir_gi.slang): после «пути 3D» пиксель берёт свой запас отражённого света и
// запасы соседей (слой 1), проверяет выбранную точку лучом видимости, итог пишет в слой 0
// (его возьмёт следующий кадр) и добавляет свет к свету пути: pc.cone — вход, pc.target —
// выход.
//
// С битом CACHE_RESOLVE — проход «кэш света: сводка» (шаг 5б, include/light_cache.slang):
// по потоку на ячейку кэша, до «пути 3D»; с CACHE_CLEAR — стереть всё (буфер новый).

[[vk::binding(2, 0)]] [format("rgba16f")] RWTexture2D<float4> images[];

#include "include/march.slang"
#include "include/shade3d.slang"
#include "include/shadow3d.slang"
#include "include/trace3d.slang"
#include "include/light3d.slang"
#include "include/restir.slang"
#include "include/restir_gi.slang"
#include "include/light_cache.slang"

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    return float4(0.0, 0.0, 0.0, 0.0);
}

static const uint RESTIR_GI_NEIGHBORS = 1;
static const uint CACHE_RESOLVE = 2;
static const uint CACHE_CLEAR = 4;
// Ячеек кэша в строке сетки запуска сводки.
static const uint CACHE_ROW = 2048;
// Соседи — не дальше стольких метров по поверхности.
static const float GI_REACH = 0.5;

void gi_neighbors(int2 px, uint w, uint h)
{
    seed_rng(uint2(px), pc.meshes, 2u);
    float4 light = textures[pc.cone].Load(int3(px, 0));
    float4 g = gbuffer_load(px);
    if (g.x < 0.0)
    {
        // Небо: свет пути как есть.
        images[pc.target][px] = light;
        return;
    }
    float2 screen = float2((float(px.x) + 0.5) / float(w) * 2.0 - 1.0, 1.0 - (float(px.y) + 0.5) / float(h) * 2.0);
    Ray ray = screen_ray(screen);
    Ray next = screen_ray(screen + float2(2.0 / float(w), 0.0));
    float spread = length(next.dir - ray.dir);
    float3 p = ray.origin + ray.dir * g.x;
    float3 n = oct_decode(g.zw);
    float3 q = p + n * (0.01 + 0.002 * g.x);
    GiBuilder b = gi_builder(q, n);
    gi_merge(b, gi_load(1u, px, w, h), 1e30, q, g.x);
    float radius = clamp(GI_REACH / max(spread * g.x, 1e-5), 2.0, GI_RADIUS);
    for (uint k = 0; k < GI_NEIGHBORS; k++)
    {
        float r = radius * sqrt(rnd());
        float phi = 2.0 * PI * rnd();
        int2 o = px + int2(round(float2(cos(phi), sin(phi)) * r));
        if (all(o == px) || any(o < 0) || o.x >= int(w) || o.y >= int(h))
            continue;
        gi_merge(b, gi_load(1u, o, w, h), GI_HISTORY, q, g.x);
    }
    GiReservoir chosen = gi_finish(b);
    if (chosen.W > 0.0)
    {
        // Точку мог закрыть предмет (чужая точка или прошлый кадр): тогда её свет не идёт.
        float3 to = chosen.pos - q;
        float d = length(to);
        if (d < 1e-4 || occluded(q, to / d, max(d * 0.98 - 0.05, 0.0)))
            chosen.W = 0.0;
    }
    // Рассеянная доля поверхности (у блестящих часть света — отражение, его несёт путь).
    float shine = material_shine(gi_material(g.y));
    float share = shine > 0.0 ? reflect_share(shine, n, ray.dir) : 0.0;
    float3 add = gi_light(chosen, q, n) * (1.0 - share);
    if (any(isnan(add)) || any(isinf(add)))
        add = float3(0.0, 0.0, 0.0);
    chosen.M = min(chosen.M, GI_HISTORY);
    gi_store(0u, px, w, h, chosen);
    images[pc.target][px] = float4(light.rgb + add, light.a);
}

[shader("compute")]
[numthreads(8, 8, 1)]
void cs_main(uint3 id : SV_DispatchThreadID)
{
    uint w, h;
    images[pc.target].GetDimensions(w, h);
    if ((pc.flags & CACHE_RESOLVE) != 0)
    {
        // Сетка запуска — CACHE_SIZE ячеек строками по CACHE_ROW, картинка — только для размера.
        uint entry = id.y * CACHE_ROW + id.x;
        if (id.x < CACHE_ROW && entry < CACHE_SIZE)
            cache_resolve(cache_base(w, h), entry, (pc.flags & CACHE_CLEAR) != 0);
        return;
    }
    if (id.x >= w || id.y >= h)
        return;
    int2 px = int2(id.xy);
    if ((pc.flags & RESTIR_GI_NEIGHBORS) != 0)
    {
        gi_neighbors(px, w, h);
        return;
    }
    seed_rng(id.xy, pc.meshes, 1u);
    float4 g = gbuffer_load(px);
    if (g.x < 0.0)
    {
        reservoir_store(1u, px, w, h, reservoir_empty());
        return;
    }
    float2 screen = float2((float(px.x) + 0.5) / float(w) * 2.0 - 1.0, 1.0 - (float(px.y) + 0.5) / float(h) * 2.0);
    Ray ray = screen_ray(screen);
    Ray next = screen_ray(screen + float2(2.0 / float(w), 0.0));
    float spread = length(next.dir - ray.dir);
    float3 p = ray.origin + ray.dir * g.x;
    float3 n = oct_decode(g.zw);
    float3 side, up;
    basis(ray.dir, side, up);
    float width = spread * g.x;
    SurfaceMaterial sm = surface_material(p, ray.dir, g.y, n, side * width, up * width, px, -1, false);
    float3 q = p + n * (0.01 + 0.002 * g.x);
    ReservoirBuilder b = builder_new(p, n);
    uint total = lamp_count();
    if (total > 0u)
    {
        for (uint k = 0; k < RESTIR_CANDIDATES; k++)
        {
            uint light = pick_lamp(rnd());
            float2 uv = float2(rnd(), rnd());
            LightSample s = light_sample(light, uv, q, n, ray.dir, sm);
            float target = luminance(s.light);
            // Вес — свет здесь, делённый на вероятность выбора (1 / число ламп).
            builder_add(b, light, uv, target, target * float(total), 1.0);
        }
        int2 prev;
        if (previous_pixel(p, w, h, PREV_RESTIR, prev))
        {
            Reservoir old = reservoir_load(0u, prev, w, h);
            builder_merge(b, old, RESTIR_HISTORY * float(RESTIR_CANDIDATES), q, ray.dir, sm, g.x);
        }
    }
    reservoir_store(1u, px, w, h, builder_finish(b));
}

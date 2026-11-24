// Сцена «3D», проход «ReSTIR: кандидаты» (путь ReSTIR, шаг 3, include/restir.slang): у
// каждого пикселя режима «качество» — свой запас точки света. Пиксель пробует
// RESTIR_CANDIDATES случайных точек ламп (без лучей тени: только «сколько света
// дала бы»), потом берёт запас своего места в прошлом кадре. Итог — в слой 1 буфера
// запасов; соседей и луч тени берёт проход «путь 3D».
//
// Константы: pc.gbuffer — буфер геометрии, pc.edges — буфер запасов, pc.meshes — номер
// кадра, pc.target — картинка того же размера (только чтобы узнать размер).

[[vk::binding(2, 0)]] [format("rgba16f")] RWTexture2D<float4> images[];

#include "include/march.slang"
#include "include/shade3d.slang"
#include "include/shadow3d.slang"
#include "include/trace3d.slang"
#include "include/light3d.slang"
#include "include/restir.slang"

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
    images[pc.target].GetDimensions(w, h);
    if (id.x >= w || id.y >= h)
        return;
    int2 px = int2(id.xy);
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
        if (previous_pixel(p, w, h, prev))
        {
            Reservoir old = reservoir_load(0u, prev, w, h);
            builder_merge(b, old, RESTIR_HISTORY * float(RESTIR_CANDIDATES), q, ray.dir, sm, g.x);
        }
    }
    reservoir_store(1u, px, w, h, builder_finish(b));
}

// Сцена «3D», проход «края 3D: тени», часть вторая (8.25): лучи теней краёв по общему
// списку работ «пиксель + доля», который собрала часть первая
// (raymarch_edge_shadows.slang). Каждый поток — одна работа: точная доля света точки
// пикселя (солнце, углы, лампа 1 или лампа 2). Её значение — в доли света пикселя
// (include/edges.slang, edge_parts_at); «края 3D» (raymarch_edges.slang) читают их оттуда.

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

[shader("compute")]
[numthreads(64, 1, 1)]
void cs_main(uint3 id : SV_DispatchThreadID)
{
    uint probe = pc.edges[6] & 0xff;
    if (probe == EDGE_PROBE_EMPTY || probe == EDGE_PROBE_NO_EXACT_SHADOWS)
        return;
    uint w, h;
    images[pc.target].GetDimensions(w, h);
    uint wh = w * h;
    if (id.x < pc.edges[EDGE_TASK_COUNT_WORD])
    {
        uint task = pc.edges[edge_tasks_at(wh) + id.x];
        uint slot = task >> 2;
        uint part = task & 3;
        // Место — обратно в пиксель: сначала ненадёжные с четырьмя лучами, потом с двумя.
        uint n4 = pc.edges[EDGE_COUNT_WORD[EDGE_U4]];
        uint packed = slot < n4 ? pc.edges[edge_list_at(EDGE_U4, slot, wh)]
                                : pc.edges[edge_list_at(EDGE_U2, slot - n4, wh)];
        int2 p = int2(packed & 0xffff, packed >> 16);
        float4 g = gbuffer_load(p);
        float2 c = float2(p) + 0.5;
        Ray r0 = screen_ray(float2(c.x / float(w) * 2.0 - 1.0, 1.0 - c.y / float(h) * 2.0));
        float value = shadow_part(r0.origin + r0.dir * g.x, oct_decode(g.zw), g.x, part);
        pc.edges[edge_parts_at(wh) + slot * 4 + part] = asuint(value);
    }
    counters_flush(PASS_EDGE_SHADOWS, id.x >> 6);
}

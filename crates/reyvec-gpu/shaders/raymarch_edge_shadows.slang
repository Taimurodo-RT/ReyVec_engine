// Сцена «3D», проход «края 3D: тени», часть первая (8.22, 8.25): какие точные доли света
// (солнце, углы, две лампы) нужны ненадёжным пикселям края. Картинка теней вдвое меньше
// кадра, и на перемычке тоньше двух пикселей или на полоске пола у края формы такой
// пиксель получал бы из неё тень соседней поверхности (7.6с, 7.19а).
//
// Группа берёт 64 ненадёжных пикселя подряд (EDGE_SHADOW_PIXELS): сначала с четырьмя
// лучами, потом с двумя; число групп — в слове EDGE_SHADOW_GROUPS_WORD заголовка. Поток
// пикселя решает, какие доли можно взять из картинки теней (8.2б, shadow_steady), и сразу
// пишет их в доли света пикселя (include/edges.slang, edge_parts_at). Остальные доли
// группа кладёт в общий список работ — подряд по долям (лампы, лампы, углы, солнце), и
// лучи соседних работ одинаковы. Лучи по списку пускает часть вторая
// (raymarch_edge_shadow_rays.slang): каждый поток — одна работа, без пустых мест.
//
// Прежде (8.22–8.24) лучи пускала сама группа: большинство долей берётся готовыми, и
// у группы из 64 потоков работ было то 20, то 130 — часть потоков ждала (на 5080 в
// заводе тени краёв 1,03 мс при 1,11 у теней всего кадра, где лучей втрое больше).

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

// Насколько могут расходиться точки картинки теней, чтобы долю взять из неё (8.2б).
static const float STEADY_SPREAD = 0.03;

// Какие доли света точки пикселя `px` (глубина `t`, нормаль `n`) можно взять из картинки
// теней (8.2б), битами по номеру доли; `value` — их значения. Пиксель ненадёжный: среди
// четырёх ближайших точек теней мало похожих на него, и обычное растяжение могло бы дать
// ему тень соседней поверхности. Но если во всём квадрате 4×4 вокруг доля одна и та же
// (у любой поверхности, и у неба: там 1), граница тени тут не проходит, и своя точка
// почти наверняка получит то же. Ещё нужна хоть одна точка своей поверхности в квадрате:
// перемычку тоньше двух пикселей картинка теней может пропустить целиком, и тогда её
// освещённость не видна ни в одной точке.
uint shadow_steady(int2 px, float t, float3 n, out float4 value)
{
    uint sw, sh;
    textures[shadow_slot()].GetDimensions(sw, sh);
    int2 gsize = gbuffer_size();
    int2 base = px >> 1;
    float4 lo = float4(1e9, 1e9, 1e9, 1e9);
    float4 hi = -lo;
    float4 sum = float4(0.0, 0.0, 0.0, 0.0);
    bool own = false;
    for (uint k = 0; k < 16; k++)
    {
        int2 hp = clamp(base + int2(int(k & 3) - 1, int(k >> 2) - 1), int2(0, 0), int2(sw, sh) - 1);
        float4 v = textures[shadow_slot()].Load(int3(hp, 0));
        lo = min(lo, v);
        hi = max(hi, v);
        sum += v;
        float4 g = gbuffer_load(min(hp * 2, gsize - 1));
        if (g.x >= 0.0)
        {
            float depth = saturate(1.0 - abs(g.x - t) / (0.02 * t + 0.05));
            float facing = saturate(dot(oct_decode(g.zw), n));
            facing *= facing;
            own = own || depth * facing * facing > 0.25;
        }
    }
    value = sum / 16.0;
    if (!own)
        return 0;
    uint mask = 0;
    for (uint p = 0; p < 4; p++)
    {
        if (hi[p] - lo[p] < STEADY_SPREAD)
            mask |= 1u << p;
    }
    return mask;
}


// Очереди точных долей (8.2б): сколько пикселей ждут каждую долю и какие. Доли идут в
// порядке PART_ORDER: потоки подряд берут одну долю, и лучи соседних потоков одинаковы
// (7.19б: при четырёх лучах потоки 0–15 — первая лампа, 16–31 — вторая, 32–47 — углы,
// 48–63 — солнце).
groupshared uint need_count[4];
groupshared uint task_base;
groupshared uint need_pixel[4][EDGE_SHADOW_PIXELS];
static const uint PART_ORDER[4] = { 2, 3, 1, 0 };

[shader("compute")]
[numthreads(64, 1, 1)]
void cs_main(uint3 id : SV_DispatchThreadID, uint lane : SV_GroupIndex)
{
    uint probe = pc.edges[6] & 0xff;
    if (probe == EDGE_PROBE_EMPTY || probe == EDGE_PROBE_NO_EXACT_SHADOWS)
        return;
    uint w, h;
    images[pc.target].GetDimensions(w, h);
    uint group = id.x >> 6;
    uint list = EDGE_R4;
    uint first_j = 0;
    uint count = 0;
    {
        uint before = 0;
        for (uint o = 0; o < 2; o++)
        {
            uint k = o == 0 ? EDGE_U4 : EDGE_U2;
            uint n = pc.edges[EDGE_COUNT_WORD[k]];
            uint groups_k = (n + EDGE_SHADOW_PIXELS - 1) / EDGE_SHADOW_PIXELS;
            if (group < before + groups_k)
            {
                list = k;
                first_j = (group - before) * EDGE_SHADOW_PIXELS;
                count = n;
                break;
            }
            before += groups_k;
        }
    }
    // Лишние группы (их не бывает, но на всякий случай) ничего не делают.
    if (list != EDGE_U4 && list != EDGE_U2)
        return;
    uint per = EDGE_SHADOW_PIXELS;
    bool reuse = (pc.edges[6] & EDGE_SHADOW_REUSE) != 0;
    if (lane < 4)
        need_count[lane] = 0;
    GroupMemoryBarrierWithGroupSync();
    bool has = lane < per && first_j + lane < count;
    uint wh = w * h;
    uint parts_at = edge_parts_at(wh);
    uint slot = edge_shadow_slot(list, first_j + lane);
    if (has)
    {
        uint packed = pc.edges[edge_list_at(list, first_j + lane, wh)];
        int2 at = int2(packed & 0xffff, packed >> 16);
        float4 g = gbuffer_load(at);
        uint steady = 0;
        float4 value = float4(1.0, 1.0, 1.0, 1.0);
        if (g.x >= 0.0 && reuse)
            steady = shadow_steady(at, g.x, oct_decode(g.zw), value);
        for (uint s = 0; s < 4; s++)
        {
            uint part = PART_ORDER[s];
            if (g.x < 0.0)
                pc.edges[parts_at + slot * 4 + part] = asuint(1.0);
            else if ((steady & (1u << part)) != 0)
                pc.edges[parts_at + slot * 4 + part] = asuint(value[part]);
            else
            {
                uint index;
                InterlockedAdd(need_count[s], 1u, index);
                need_pixel[s][index] = lane;
            }
        }
    }
    GroupMemoryBarrierWithGroupSync();
    uint total = need_count[0] + need_count[1] + need_count[2] + need_count[3];
    if (lane == 0 && total > 0)
    {
        uint base;
        InterlockedAdd(pc.edges[EDGE_TASK_COUNT_WORD], total, base);
        task_base = base;
        // Новые границы групп по 64 работы — в размер запуска второй части.
        uint crossed = (base + total + 63) / 64 - (base + 63) / 64;
        if (crossed > 0)
            InterlockedAdd(pc.edges[EDGE_TASK_GROUPS_WORD], crossed);
    }
    GroupMemoryBarrierWithGroupSync();
    uint tasks_at = edge_tasks_at(wh);
    for (uint i = lane; i < total; i += 64)
    {
        uint s = 0;
        uint index = i;
        while (s < 3 && index >= need_count[s])
        {
            index -= need_count[s];
            s++;
        }
        uint mine = need_pixel[s][index];
        uint slot_m = edge_shadow_slot(list, first_j + mine);
        pc.edges[tasks_at + task_base + i] = (slot_m << 2) | PART_ORDER[s];
    }
}

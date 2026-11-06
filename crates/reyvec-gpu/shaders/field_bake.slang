// Сцена «3D», проход «поле» (7.10б): считает кубики запечённого поля расстояний
// (docs/decisions/0019-baked-distance-field.md). Зеркало Field::update в
// crates/reyvec-sdf/src/field3d.rs: те же точки, та же полоса, то же правило шаров-границ.
//
// Задания кадра (`pc.jobs`, пишет процессор, раскладка — space::pack_field_jobs):
// [0] — сколько кубиков, [1] — где их записи, [2] — где формы (список слов pack_scene:
// объекты и программы), [3] — точек по стороне слоя. Запись кубика — 8 слов: слой,
// мировой номер первой точки (x, y, z, со знаком), начало и длина списка предметов
// (номера объектов форм), 2 запасных.
//
// Поле (`pc.field`): слои подряд, в слое size³ чисел half float, по два в слове (чётная
// точка по x — младшие 16 бит). Точка с мировым номером w лежит в ячейке w mod size.
//
// Группа из 256 потоков — один кубик 8×8×8; поток считает две соседние по x точки.

#include "include/sdf3d.slang"

struct Push
{
    uint* jobs;
    uint* field;
};
[[vk::push_constant]] ConstantBuffer<Push> pc;

// Как shape3d::OBJECT_WORDS и MAX_PROGRAM_WORDS в space.slang.
static const uint BAKE_OBJECT_WORDS = 8;
static const uint BAKE_MAX_PROGRAM_WORDS = 16384;
// Как field3d::FIELD_STEP, BRICK.
static const float FIELD_STEP = 1.0 / 32.0;
static const int FIELD_BRICK = 8;

// Файл обязан иметь пару для рисования (build.rs собирает её у всех шейдеров); здесь
// она не используется.
struct VertexOut
{
    float4 position : SV_Position;
};

[shader("vertex")]
VertexOut vs_main(uint vertex : SV_VulkanVertexID)
{
    VertexOut o;
    o.position = float4(0.0, 0.0, 0.0, 1.0);
    return o;
}

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    return float4(0.0, 0.0, 0.0, 0.0);
}

// Полоса слоя: как field3d::band.
float field_band(uint level)
{
    return max(8.0 * FIELD_STEP * float(1u << level), 0.5);
}

// Расстояние до форм списка в точке `p`, как field3d::solid_distance: дальше `near` от
// шара-границы ответ — расстояние до шара.
float bake_distance(uint* scene, uint list, uint count, float3 p, float near)
{
    uint objects = scene[1];
    float best = 1e9;
    for (uint k = 0; k < count; k++)
    {
        uint w = objects + pc.jobs[list + k] * BAKE_OBJECT_WORDS;
        float radius = asfloat(scene[w + 7]);
        if (radius >= 0.0)
        {
            float3 center = float3(asfloat(scene[w + 4]), asfloat(scene[w + 5]), asfloat(scene[w + 6]));
            float outside = length(p - center) - radius;
            if (outside >= best)
                continue;
            if (outside > near)
            {
                best = outside;
                continue;
            }
        }
        uint start = scene[w];
        uint len = min(scene[w + 1], BAKE_MAX_PROGRAM_WORDS);
        uint head = scene[start];
        float d;
        if (len == PRIM_WORDS + (head >> 16) && (head & 0xff) == OP_PRIM)
            d = sdf_primitive(scene, start, p).x;
        else if ((scene[w + 2] & 4) != 0) // OBJECT_CHAIN
            d = sdf_chain(scene, start, len, p).x;
        else
            d = sdf_program(scene, start, len, p).x;
        best = min(best, d);
    }
    return best;
}

[shader("compute")]
[numthreads(256, 1, 1)]
void cs_main(uint3 group : SV_GroupID, uint3 thread : SV_GroupThreadID)
{
    uint count = pc.jobs[0];
    if (group.x >= count)
        return;
    uint job = pc.jobs[1] + group.x * 8;
    uint* scene = pc.jobs + pc.jobs[2];
    int size = int(pc.jobs[3]);
    uint level = min(pc.jobs[job], 3u);
    int3 first = int3(int(pc.jobs[job + 1]), int(pc.jobs[job + 2]), int(pc.jobs[job + 3]));
    uint list = pc.jobs[job + 4];
    uint list_count = pc.jobs[job + 5];

    uint t = thread.x;
    int3 o = int3(int(t & 3) * 2, int((t >> 2) & 7), int(t >> 5));
    float step = FIELD_STEP * float(1u << level);
    float band = field_band(level);
    float2 pair;
    for (int i = 0; i < 2; i++)
    {
        int3 w = first + o + int3(i, 0, 0);
        float d = bake_distance(scene, list, list_count, float3(w) * step, band);
        pair[i] = clamp(d, -band, band);
    }
    // Как packHalf2x16 в GLSL: два half float в слове без 16-битных типов в шейдере
    // (их умеют не все видеокарты).
    uint halves = packHalf2x16(pair);
    int3 q = ((first + o) % size + size) % size;
    uint slot = uint((q.z * size + q.y) * size + q.x);
    uint cells = uint(size * size * size);
    pc.field[(level * cells + slot) >> 1] = halves;
}

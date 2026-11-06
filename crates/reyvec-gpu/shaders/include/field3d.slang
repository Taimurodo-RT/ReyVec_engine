// Запечённое поле расстояний вокруг камеры (7.10, docs/decisions/0019-baked-distance-field.md):
// чтение. Зеркало Field::lower_bound в crates/reyvec-sdf/src/field3d.rs, считает его
// проход «поле» (field_bake.slang). Перед подключением — space.slang (pc.scene).
//
// Раздел поля в сцене кадра (space::set_field): слово 10 заголовка — где он (0 — поля
// нет); адрес буфера поля (8 байт), точек по стороне слоя, 1 запасное, углы слоёв (по 3
// целых, мировой номер первой точки). Буфер: слои подряд, по size³ half float, два
// в слове; точка w слоя лежит в ячейке w mod size.

// Как field3d::FIELD_STEP, FIELD_LEVELS, INTERP_MARGIN.
static const float FIELD_STEP_3D = 1.0 / 32.0;
static const uint FIELD_LEVELS_3D = 4;
static const float FIELD_INTERP_MARGIN = 0.87;

// Указатель на буфер поля лежит в сцене кадра: так его можно прочитать без 64-битных
// целых в шейдере.
struct FieldRef
{
    uint* values;
};

// Полоса слоя: как field3d::band.
float field_band_3d(uint level)
{
    return max(8.0 * FIELD_STEP_3D * float(1u << level), 0.5);
}

// Число в точке с мировым номером `w` слоя `level`.
float field_value_3d(uint* values, int size, uint level, int3 w)
{
    int3 q = (w % size + size) % size;
    uint slot = uint((q.z * size + q.y) * size + q.x);
    uint cells = uint(size * size * size);
    float2 pair = unpackHalf2x16ToFloat(values[(level * cells + slot) >> 1]);
    return (slot & 1) != 0 ? pair.y : pair.x;
}

// Расстояние до форм без плоскостей в точке `p` не меньше этого. Отрицательная
// бесконечность (-1e30) — поля нет или точка вне всех слоёв: тогда оценки нет.
// Дальше полосы слоя оценка не растёт: поле там знает только «не ближе полосы».
float field_lower(float3 p)
{
    float estimate;
    return field_lower(p, estimate);
}

// То же, и в `estimate` — само число поля (выборка без запаса): ближе к настоящему
// расстоянию, но может быть и больше него.
float field_lower(float3 p, out float estimate)
{
    estimate = -1e30;
    uint at = pc.scene[10];
    if (at == 0)
        return -1e30;
    uint* values = ((FieldRef*)(pc.scene + at))->values;
    int size = int(pc.scene[at + 2]);
    for (uint level = 0; level < FIELD_LEVELS_3D; level++)
    {
        uint c = at + 4 + level * 3;
        int3 corner = int3(int(pc.scene[c]), int(pc.scene[c + 1]), int(pc.scene[c + 2]));
        float step = FIELD_STEP_3D * float(1u << level);
        float3 g = p / step;
        float3 base = floor(g);
        int3 b = int3(base);
        int3 rel = b - corner;
        if (any(rel < 0) || any(rel >= size - 1))
            continue;
        float3 f = g - base;
        float v = 0.0;
        for (uint k = 0; k < 8; k++)
        {
            int3 o = int3(int(k & 1), int((k >> 1) & 1), int(k >> 2));
            float3 w = lerp(1.0 - f, f, float3(o));
            v += w.x * w.y * w.z * field_value_3d(values, size, level, b + o);
        }
        estimate = v;
        return v - FIELD_INTERP_MARGIN * step - field_band_3d(level) * (1.0 / 1024.0);
    }
    return -1e30;
}

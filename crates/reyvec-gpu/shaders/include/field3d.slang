// Запечённое поле расстояний вокруг камеры (7.10, docs/decisions/0019-baked-distance-field.md):
// чтение. Зеркало Field::lower_bound в crates/reyvec-sdf/src/field3d.rs, считает его
// проход «поле» (field_bake.slang). Перед подключением — space.slang (pc.scene).
//
// Раздел поля в сцене кадра (space::set_field): слово 10 заголовка — где он (0 — поля
// нет); адрес буфера поля (8 байт, здесь не читается), точек по стороне слоя, 1 запасное,
// углы слоёв (по 3 целых, мировой номер первой точки). Слой: точка w лежит в ячейке
// w mod size. Проход «поле» считает кубики в буфер и копирует их в слои-картинки.

// Как field3d::FIELD_STEP, FIELD_LEVELS, INTERP_MARGIN.
static const float FIELD_STEP_3D = 1.0 / 32.0;
static const uint FIELD_LEVELS_3D = 4;
static const float FIELD_INTERP_MARGIN = 0.87;

// Слои поля — объёмные картинки R16F (7.10е, bindless binding 3), сэмплер — сглаживающий с
// повтором по краям (binding 4): повтор и есть кольцевая раскладка «точка w в ячейке
// w mod size». Одна выборка с аппаратным смешиванием вместо восьми чтений из буфера.
[[vk::binding(3, 0)]] Texture3D<float> field_volumes[FIELD_LEVELS_3D];
[[vk::binding(4, 0)]] SamplerState field_sampler;

// Полоса слоя: как field3d::band.
float field_band_3d(uint level)
{
    return max(8.0 * FIELD_STEP_3D * float(1u << level), 0.5);
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
    int size = int(pc.scene[at + 2]);
    float inv_size = 1.0 / float(size);
    for (uint level = 0; level < FIELD_LEVELS_3D; level++)
    {
        uint c = at + 4 + level * 3;
        int3 corner = int3(int(pc.scene[c]), int(pc.scene[c + 1]), int(pc.scene[c + 2]));
        float step = FIELD_STEP_3D * float(1u << level);
        float3 g = p / step;
        int3 rel = int3(floor(g)) - corner;
        if (any(rel < 0) || any(rel >= size - 1))
            continue;
        // Точка с мировым номером w — середина ячейки (w mod size): (w + 0.5) / size.
        float v = field_volumes[NonUniformResourceIndex(level)].SampleLevel(
            field_sampler, (g + 0.5) * inv_size, 0.0);
        estimate = v;
        // Запас: 0.87 шага на смешивание (как field3d::INTERP_MARGIN), полоса/1024 на
        // округление до half float и 1/64 шага на грубые веса аппаратного смешивания
        // (8 бит дробной части; соседние точки отличаются не больше чем на √3 шага).
        return v - (FIELD_INTERP_MARGIN + 1.0 / 64.0) * step - field_band_3d(level) * (1.0 / 1024.0);
    }
    return -1e30;
}

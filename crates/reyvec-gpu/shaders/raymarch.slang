// Сцена «3D», проход «реймарчинг» (этап 7.3): каждая точка экрана пускает луч от камеры
// и шагает по нему на расстояние до ближайшей поверхности (sphere tracing), пока не
// упрётся в неё. Формы приходят данными (7.2, include/sdf3d.slang). Результат — буфер
// геометрии, по точке на пиксель: расстояние вдоль луча, материал и нормаль. Освещает
// его следующий проход (raymarch_shade.slang), поэтому свет и тени (7.4) не платят за
// шагание ещё раз, а время шагания видно на панели отдельной строкой.
//
// Точка буфера (RGBA32F): x — расстояние от ближней плоскости вдоль луча (-1 — луч ушёл
// в небо), y — номер материала, zw — нормаль (oct_encode).

// Длины, с которых начинать лучи (7.8, raymarch_cone.slang): точка на квадрат 8×8,
// номер картинки в поле `cone` констант. Буфер геометрии полигональных моделей (7.6ж,
// mesh3d.slang) — в поле `meshes`, если стоит флаг SPACE_FLAG_MESHES. Сам список
// `textures[]` объявлен в include/textures.slang (через sdf3d.slang).
// Запись в картинки (binding 2 общего списка): сюда вычислительный вариант кладёт буфер
// геометрии, номер — в поле `target` констант.
[[vk::binding(2, 0)]] [format("rgba32f")] RWTexture2D<float4> images[];

#include "include/march.slang"

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    float4 g = march_pixel(int2(input.position.xy), input.screen, -1, input.screen);
    counters_flush_fragment(PASS_MARCH, int2(input.position.xy));
    return g;
}

// Опыт 7.6м: те же лучи вычислительным шейдером. Группа 8×8 — ровно квадрат конуса
// (все её лучи начинают с одной длины). Результат пишется прямо в картинку буфера
// геометрии, без копии.
[shader("compute")]
[numthreads(8, 8, 1)]
void cs_main(uint3 id : SV_DispatchThreadID)
{
    uint w, h;
    images[pc.target].GetDimensions(w, h);
    if (id.x >= w || id.y >= h)
        return;
    float2 screen = float2((float(id.x) + 0.5) / float(w) * 2.0 - 1.0, 1.0 - (float(id.y) + 0.5) / float(h) * 2.0);
    images[pc.target][id.xy] = march_pixel(int2(id.xy), screen, -1, screen);
    counters_flush(PASS_MARCH, counter_spread(int2(id.xy)));
}

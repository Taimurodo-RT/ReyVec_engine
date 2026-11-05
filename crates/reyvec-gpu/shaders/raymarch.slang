// Сцена «3D», проход «реймарчинг» (этап 7.3): каждая точка экрана пускает луч от камеры
// и шагает по нему на расстояние до ближайшей поверхности (sphere tracing), пока не
// упрётся в неё. Формы приходят данными (7.2, include/sdf3d.slang). Результат — буфер
// геометрии, по точке на пиксель: расстояние вдоль луча, материал и нормаль. Освещает
// его следующий проход (raymarch_shade.slang), поэтому свет и тени (7.4) не платят за
// шагание ещё раз, а время шагания видно на панели отдельной строкой.
//
// Точка буфера (RGBA32F): x — расстояние от ближней плоскости вдоль луча (-1 — луч ушёл
// в небо), y — номер материала, zw — нормаль (oct_encode).

#include "include/space.slang"

float3 normal_at(float3 p)
{
    // Наклон поля по четырём точкам тетраэдра: дешевле шести и без перекоса. Точки
    // перебираются циклом, а не четырьмя вызовами подряд: так scene() встраивается в
    // шейдер один раз, и драйверы (особенно программный lavapipe) собирают его быстро.
    const float e = 0.001;
    float3 n = float3(0.0, 0.0, 0.0);
    for (uint k = 0; k < 4; k++)
    {
        float3 dir = float3(float(((k + 3) >> 1) & 1), float((k >> 1) & 1), float(k & 1)) * 2.0 - 1.0;
        n += dir * scene(p + dir * e).x;
    }
    return normalize(n);
}

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    Ray ray = screen_ray(input.screen);

    // Шагаем по лучу: каждый шаг — на расстояние до ближайшей формы, ближе неё ничего нет.
    float t = 0.0;
    float2 hit = float2(-1.0, 0.0);
    for (int i = 0; i < 160; i++)
    {
        float2 s = scene(ray.origin + ray.dir * t);
        // Точность растёт к камере: половина миллиметра на метр пути.
        if (s.x < 0.0005 * max(t, 1.0))
        {
            hit = float2(t, s.y);
            break;
        }
        t += s.x;
        if (t > ray.limit)
            break;
    }
    if (hit.x < 0.0)
        return float4(-1.0, 0.0, 0.0, 0.0);
    float3 n = normal_at(ray.origin + ray.dir * hit.x);
    return float4(hit.x, hit.y, oct_encode(n));
}

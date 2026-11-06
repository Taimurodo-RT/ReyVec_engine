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

// Длины, с которых начинать лучи (7.8, raymarch_cone.slang): точка на квадрат 8×8.
// Номер картинки приходит в поле `gbuffer` констант. Буфер геометрии полигональных
// моделей (7.6ж, mesh3d.slang) — в поле `shadows`, если стоит флаг SPACE_FLAG_MESHES.
[[vk::binding(0, 0)]] Texture2D textures[];
// Запись в картинки (binding 2 общего списка): сюда вычислительный вариант кладёт буфер
// геометрии, номер — в поле `target` констант.
[[vk::binding(2, 0)]] [format("rgba32f")] RWTexture2D<float4> images[];

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
        n += dir * scene_near(p + dir * e, 1e9, false, true).x;
    }
    return normalize(n);
}

// Точка буфера геометрии для пикселя `px` (точка экрана `screen`). Общая для
// пиксельного шейдера и вычислительного (опыт 7.6м).
float4 march_pixel(int2 px, float2 screen)
{
    Ray ray = screen_ray(screen);

    // Шагаем по лучу: каждый шаг — на расстояние до ближайшей формы, ближе неё ничего нет.
    // Начало — там, куда дошёл конус квадрата 8×8 (до этого места пусто для всех его
    // лучей).
    // Полигоны (7.6ж) нарисованы раньше: дальше их точки луч не идёт, а если ни во что
    // не попал раньше — пиксель их.
    float4 mesh = float4(-1.0, 0.0, 0.0, 0.0);
    if ((pc.flags & SPACE_FLAG_MESHES) != 0)
        mesh = textures[pc.shadows].Load(int3(px, 0));
    float limit = mesh.x >= 0.0 ? min(mesh.x, ray.limit) : ray.limit;
    // Плоскости (пол, стены) — формулой (7.6и): луч шагает только среди остальных форм
    // и не дальше точки плоскости; не встретив ничего раньше, пиксель берёт её.
    float3 plane_normal;
    float2 plane = plane_hit(ray.origin, ray.dir, plane_normal);
    bool plane_first = plane.x <= limit;
    limit = min(limit, plane.x);
    float t = min(textures[pc.gbuffer].Load(int3(px / 8, 0)).x, limit);
    float2 hit = float2(-1.0, 0.0);
    // Шаг с «перелётом» (over-relaxation, Keinert и др., «Enhanced Sphere Tracing»,
    // 2014): шагаем в 1.6 раза дальше безопасного. Пока шары пустоты соседних точек
    // перекрываются, между ними точно пусто и перелёт законен. Если не перекрылись —
    // мы могли проскочить поверхность: откатываемся на последний шаг и дальше идём
    // обычными шагами. Вдоль пола и боков предметов (луч скользит почти параллельно)
    // это экономит пятую часть шагов (замер на процессоре по сцене «3D»).
    float omega = 1.6;
    float step = 0.0;
    float previous = 0.0;
    for (int i = 0; i < 160; i++)
    {
        // Дальше 25 см (и ещё миллиметр на метр пути) от границы объекта его форма не
        // считается: это всегда больше точности попадания ниже.
        float2 s = scene_near(ray.origin + ray.dir * t, 0.25 + 0.001 * t, false, false);
        float radius = abs(s.x);
        if (omega > 1.0 && radius + previous < step)
        {
            // Могли перелететь: назад, на обычный безопасный шаг от прошлой точки
            // (её пустота `previous` уже посчитана), дальше без перелёта.
            t -= step - previous;
            step = previous;
            omega = 1.0;
            continue;
        }
        // Точность растёт к камере: половина миллиметра на метр пути.
        if (s.x < 0.0005 * max(t, 1.0))
        {
            hit = float2(t, s.y);
            break;
        }
        step = s.x * omega;
        previous = radius;
        t += step;
        if (t > limit)
            break;
    }
    if (hit.x < 0.0)
    {
        if (plane_first)
            return float4(plane.x, plane.y, oct_encode(plane_normal));
        return mesh;
    }
    float3 n = normal_at(ray.origin + ray.dir * hit.x);
    return float4(hit.x, hit.y, oct_encode(n));
}

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    return march_pixel(int2(input.position.xy), input.screen);
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
    images[pc.target][id.xy] = march_pixel(int2(id.xy), screen);
}

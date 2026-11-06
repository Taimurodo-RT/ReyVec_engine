// Шагание луча глаза (7.3): общее для прохода «реймарчинг» (raymarch.slang) и краёв
// форм (7.6о, raymarch_edges.slang). Перед подключением должен быть объявлен
// `textures[]`. Начальные длины — `textures[pc.cone]`, полигоны — `textures[pc.meshes]`.

#include "space.slang"

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
        mesh = textures[pc.meshes].Load(int3(px, 0));
    float limit = mesh.x >= 0.0 ? min(mesh.x, ray.limit) : ray.limit;
    // Плоскости (пол, стены) — формулой (7.6и): луч шагает только среди остальных форм
    // и не дальше точки плоскости; не встретив ничего раньше, пиксель берёт её.
    float3 plane_normal;
    float2 plane = plane_hit(ray.origin, ray.dir, plane_normal);
    bool plane_first = plane.x <= limit;
    limit = min(limit, plane.x);
    float t = min(textures[pc.cone].Load(int3(px / 8, 0)).x, limit);
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
        {
            // Перелёт мог уйти за пол (или полигоны) и проскочить форму, стоящую на нём:
            // без отката луч брал точку пола под ней, в тени и в углу, — чёрные пятна
            // квадратами 8×8 у подножий форм, видные сквозь дыры (7.6р). Откатываемся
            // на безопасный шаг; останавливаемся, только если и он за пределом.
            if (omega <= 1.0)
                break;
            t -= step - previous;
            step = previous;
            omega = 1.0;
            if (t > limit)
                break;
        }
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


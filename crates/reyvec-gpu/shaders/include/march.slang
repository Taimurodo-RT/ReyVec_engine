// Шагание луча глаза (7.3): общее для прохода «реймарчинг» (raymarch.slang) и краёв
// форм (7.6о, raymarch_edges.slang). Список картинок `textures[]` приходит из
// textures.slang. Начальные длины — `textures[pc.cone]`, полигоны — `textures[pc.meshes]`.

#include "space.slang"

// Нормаль у точки попадания `p` (на расстоянии `t` по лучу). Ближе NORMAL_NEAR к точкам
// тетраэдра настоящая форма всегда есть, поэтому объекты дальше отвечают шаром-границей —
// ответ тот же до бита, что без этого. Если в первой точке ближайший объект впереди
// остальных больше чем на NORMAL_GAP, в трёх других точках он ближайший заведомо
// (расстояния меняются не быстрее чем в 4 раза быстрее точки, а точки тетраэдра — в
// 2√2·e друг от друга), и считается он один (7.18д5; зеркало Grid::normal_samples).
static const float NORMAL_NEAR = 0.02;
static const float NORMAL_GAP = 0.024;
// Бит 2 слова 11 заголовка: нормаль всегда по всей сцене (сравнение в тестах).
static const uint SCENE_FULL_NORMAL = 4;

float3 normal_at(float3 p, float t)
{
    // Наклон поля по четырём точкам тетраэдра: дешевле шести и без перекоса. Точки
    // перебираются циклом, а не четырьмя вызовами подряд: так scene() встраивается в
    // шейдер один раз, и драйверы (особенно программный lavapipe) собирают его быстро.
    const float e = 0.001;
    float near = 0.0005 * max(t, 1.0) + NORMAL_NEAR;
    bool full = (pc.scene[11] & SCENE_FULL_NORMAL) != 0;
    float3 n = float3(0.0, 0.0, 0.0);
    uint only = SCENE_ALL;
    for (uint k = 0; k < 4; k++)
    {
        float3 dir = float3(float(((k + 3) >> 1) & 1), float((k >> 1) & 1), float(k & 1)) * 2.0 - 1.0;
        count_normal();
        uint winner;
        float second;
        float d = scene_pick(p + dir * e, full || only != SCENE_ALL ? 1e9 : near, false, true, only, winner, second).x;
        if (k == 0 && !full && winner != SCENE_ALL && second - d > NORMAL_GAP)
            only = winner;
        n += dir * d;
    }
    return normalize(n);
}

// Точка буфера геометрии для пикселя `px` (точка экрана `screen`). Общая для
// пиксельного шейдера и вычислительного (опыт 7.6м).
float4 march_pixel(int2 px, float2 screen)
{
    count_ray();
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
    float3 hit_at = ray.origin;
    for (int i = 0; i < 160; i++)
    {
        count_step();
        // Дальше 25 см (и ещё миллиметр на метр пути) от границы объекта его форма не
        // считается: это всегда больше точности попадания ниже.
        float3 at = ray.origin + ray.dir * t;
        float2 s = scene_near(at, 0.25 + 0.001 * t, false, false);
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
            hit_at = at;
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
    // Материал первого слоя поля уровня — один раз, в точке попадания (7.18д3).
    if (hit.y == LAYER_MATERIAL_LOOKUP)
        hit.y = level_field_material(pc.scene, pc.scene[13], hit_at);
    float3 n = normal_at(ray.origin + ray.dir * hit.x, hit.x);
    return float4(hit.x, hit.y, oct_encode(n));
}


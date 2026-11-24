// Лучи по сцене «3D» не из камеры (Л1, эталон): отражения, отскоки пути и лучи тени
// к точке света. Перед подключением — space.slang (и sdf3d.slang с ним).

static const int TRACE_STEPS = 160;

// Шагание луча из `origin` по `dir` не дальше `far`, как march_pixel (перелёт с откатом,
// плоскости формулой), но без конусов и полигонов: видит то же, что глаз. x — длина до
// попадания (−1 — небо), y — материал; `normal` — нормаль, если попал в плоскость
// (`plane` — да).
float2 trace_eye(float3 origin, float3 dir, float far, out bool plane, out float3 normal)
{
    float2 hit_plane = plane_hit(origin, dir, normal);
    float limit = min(far, hit_plane.x);
    plane = false;
    float t = 0.0;
    float omega = 1.6;
    float step = 0.0;
    float previous = 0.0;
    for (int i = 0; i < TRACE_STEPS; i++)
    {
        count_step();
        float3 at = origin + dir * t;
        float2 s = scene_near(at, 0.25 + 0.001 * t, false, false);
        float radius = abs(s.x);
        if (omega > 1.0 && radius + previous < step)
        {
            t -= step - previous;
            step = previous;
            omega = 1.0;
            continue;
        }
        // Точность грубее, чем у луча глаза: отражение в кривой поверхности и так
        // растягивает точку на много пикселей, а отскок пути не виден глазу точкой.
        if (s.x < 0.001 * max(t, 1.0))
        {
            float material = s.y;
            if (material == LAYER_MATERIAL_LOOKUP)
                material = level_field_material(pc.scene, pc.scene[13], at);
            return float2(t, material);
        }
        step = s.x * omega;
        previous = radius;
        t += step;
        if (t > limit)
        {
            if (omega <= 1.0)
                break;
            t -= step - previous;
            step = previous;
            omega = 1.0;
            if (t > limit)
                break;
        }
    }
    if (hit_plane.x <= far)
    {
        plane = true;
        return hit_plane;
    }
    return float2(-1.0, 0.0);
}

// Закрыт ли путь из `from` по `dir` на длину `far` (луч тени с ответом «да или нет»,
// без полутени по близости форм: полутень эталона — от размера источника). Видит и
// заменители, как тени (shadow-only), плоскости — формулой.
bool occluded(float3 from, float3 dir, float far)
{
    float3 normal;
    if (plane_hit(from, dir, normal).x < far)
        return true;
    float t = 0.0;
    for (int i = 0; i < TRACE_STEPS; i++)
    {
        count_step();
        float2 s = scene_near(from + dir * t, 0.25 + 0.001 * t, true, false);
        if (s.x < 0.0005 * max(t, 1.0))
            return true;
        t += s.x;
        if (t >= far)
            return false;
    }
    return false;
}

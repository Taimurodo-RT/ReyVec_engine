// Сцена «3D», проход «лучи 3D грубо» (7.8): предварительное шагание конусами. Одна точка
// этого прохода — квадрат 8×8 пикселей кадра. Вместо тонкого луча по сцене идёт конус,
// который накрывает лучи всех 64 пикселей квадрата: шаг делается, только если шар
// пустоты вокруг оси конуса шире самого конуса. Где конус упёрся в форму (или ушёл за
// дальнюю плоскость), пишется пройденная длина: до неё ни один луч квадрата ничего не
// встретит. Проход «реймарчинг» начинает каждый луч с этой длины, а не от камеры, и
// не тратит десятки шагов на пустое место перед полом и формами.
//
// Точка результата (R32F): длина вдоль луча, с которой безопасно начать.

#include "include/space.slang"

static const float CONE_TILE = 8.0; // как CONE_TILE в crates/reyvec-gpu/src/demo.rs

// Луч через точку кадра `c` (в пикселях кадра) размером `size`.
Ray pixel_ray(float2 c, float2 size)
{
    return screen_ray(float2(c.x / size.x * 2.0 - 1.0, 1.0 - c.y / size.y * 2.0));
}

[shader("fragment")]
float fs_main(VertexOut input) : SV_Target
{
    float2 size = float2(round(pc.aspect * pc.height), pc.height);
    float2 corner = floor(input.position.xy) * CONE_TILE;
    Ray axis = pixel_ray(corner + CONE_TILE * 0.5, size);
    Ray edge = pixel_ray(corner, size);
    // Как далеко луч угла квадрата от оси в точке t: оба луча прямые, поэтому
    // расстояние меняется по прямой от `spread0` до `spread0 + spread1 · t` (для плоской
    // камеры лучи параллельны и расстояние постоянно).
    float3 offset = edge.origin - axis.origin;
    float3 drift = edge.dir - axis.dir;
    // Плоскости (7.6и) конус не опрашивает: каждый луч квадрата сам упрётся в свою
    // точку плоскости. Дальше точки плоскости на оси идти незачем — там пол.
    float3 plane_normal;
    float limit = min(axis.limit, plane_hit(axis.origin, axis.dir, plane_normal).x);
    count_ray();
    float t = 0.0;
    for (int i = 0; i < 96; i++)
    {
        count_step();
        float cone = length(offset + drift * t);
        float d = scene_near(axis.origin + axis.dir * t, max(0.25, 2.0 * cone), false, false).x;
        // Пустота должна быть шире конуса с запасом: иначе дальше идут тонкие лучи.
        float free = d - cone;
        if (free < 0.01 + 0.002 * t)
            break;
        // Шаг меньше пустоты: за шаг конус чуть расширяется.
        t += free * 0.9;
        if (t > limit)
            break;
    }
    counters_flush_fragment(PASS_CONE, int2(input.position.xy));
    return min(t, limit);
}

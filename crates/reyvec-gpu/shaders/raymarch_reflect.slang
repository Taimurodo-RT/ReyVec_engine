// Сцена «3D», проход «отражения 3D» (Л1, docs/rays-further.md): резкие отражения. Для
// каждого пикселя, где луч глаза попал на блестящий материал (блеск больше 0), луч
// отражается по нормали из буфера геометрии и шагает по тому же полю сцены, что и луч
// глаза. Точку, куда он попал, освещаем как обычную: солнце и две первые лампы с
// тенями (тот же shadow_part, что у «теней 3D»), остальные лампы без теней, свет неба
// без затенения в углах. Ушёл в небо — цвет неба. Один отскок: в зеркале зеркало
// видно матовым.
//
// Результат (RGBA16F, размер буфера геометрии): rgb — цвет отражения, a — длина луча
// отражения до попадания (небо — REFLECT_SKY), 0 — отражения нет. «Освещение 3D» и «края
// 3D» смешивают его с цветом поверхности по доле Френеля (shade_surface в
// include/shade3d.slang).
//
// Шероховатые материалы (Л2, слово 7 записи материала): отражение потом размывает
// raymarch_reflect_blur.slang, поэтому лучи у них — только в левом верхнем пикселе
// квадрата 2×2 (вчетверо меньше лучей), остальные пишут 0 и получат цвет от соседей при
// размытии. Так — только с флагом REFLECT_FLAG_ROUGH (иначе шероховатость не
// учитывается, и отражения везде резкие).
//
// Полигоны (модели из треугольников) в отражениях не видны: их нет в поле сцены.

[[vk::binding(2, 0)]] [format("rgba16f")] RWTexture2D<float4> images[];

#include "include/march.slang"
#include "include/shade3d.slang"
#include "include/shadow3d.slang"

// Файл обязан иметь пару для рисования (build.rs собирает её у всех шейдеров); здесь
// она не используется.
[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    return float4(0.0, 0.0, 0.0, 0.0);
}

// Как далеко идёт луч отражения, метров, и сколько у него шагов.
static const float REFLECT_FAR = 80.0;
static const int REFLECT_STEPS = 160;
// Длина «до неба» в альфе результата.
static const float REFLECT_SKY = 100.0;
// Флаг констант этого прохода (Л2): размытые отражения включены. Бит освещения
// SPACE_FLAG_DITHER, этому проходу он не нужен.
static const uint REFLECT_FLAG_ROUGH = 64;

// Шагание луча отражения из `origin` по `dir`, как march_pixel (перелёт с откатом,
// плоскости формулой), но без конусов и полигонов. x — длина до попадания (−1 — небо),
// y — материал; `normal` — нормаль, если попал в плоскость (`plane` — да).
float2 march_reflect(float3 origin, float3 dir, out bool plane, out float3 normal)
{
    float2 hit_plane = plane_hit(origin, dir, normal);
    float limit = min(REFLECT_FAR, hit_plane.x);
    plane = false;
    float t = 0.0;
    float omega = 1.6;
    float step = 0.0;
    float previous = 0.0;
    for (int i = 0; i < REFLECT_STEPS; i++)
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
        // растягивает точку на много пикселей.
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
    if (hit_plane.x <= REFLECT_FAR)
    {
        plane = true;
        return hit_plane;
    }
    return float2(-1.0, 0.0);
}

// Цвет того, что видно из точки `p` (нормаль `n`, луч глаза прошёл до неё `t` метров)
// в зеркальном направлении `dir`. `spread` — ширина пикселя на метр пути (для сетки пола
// в отражении).
float4 reflected(float3 p, float3 n, float3 dir, float t, float spread, int2 px)
{
    float3 from = p + n * (0.01 + 0.002 * t);
    bool plane;
    float3 normal;
    count_ray();
    float2 hit = march_reflect(from, dir, plane, normal);
    if (hit.x < 0.0)
        return float4(sky(dir), REFLECT_SKY);
    float3 at = from + dir * hit.x;
    float total = t + hit.x;
    if (!plane)
    {
        float mean;
        normal = normal_at(at, total, false, mean);
    }
    // Солнце и две первые лампы — лучами тени, углы не затеняем. Одним циклом: луч тени
    // встраивается в шейдер один раз (как в raymarch_shadow.slang).
    float shadow[4] = { 1.0, 1.0, 1.0, 1.0 };
    for (uint k = 0; k < 4; k++)
    {
        if (k == 1)
            continue;
        shadow[k] = shadow_part(at, normal, total, k);
    }
    float4 shadows = float4(shadow[0], shadow[1], shadow[2], shadow[3]);
    // Сдвиг на соседний пиксель — поперёк луча, по ширине пикселя на всём пути.
    float3 side = normalize(cross(dir, abs(dir.y) < 0.99 ? float3(0.0, 1.0, 0.0) : float3(1.0, 0.0, 0.0)));
    float3 up = cross(side, dir);
    float w = spread * total;
    float3 color = shade_surface(at, dir, total, hit.y, normal, shadows, side * w, up * w, px, -1, false);
    return float4(color, max(hit.x, 1e-3));
}

[shader("compute")]
[numthreads(8, 8, 1)]
void cs_main(uint3 id : SV_DispatchThreadID)
{
    uint w, h;
    images[pc.target].GetDimensions(w, h);
    if (id.x >= w || id.y >= h)
        return;
    int2 px = int2(id.xy);
    float4 g = gbuffer_load(px);
    float4 result = float4(0.0, 0.0, 0.0, 0.0);
    if (g.x >= 0.0)
    {
        uint material = min(uint(g.y + 0.5), max(pc.scene[3], 1) - 1);
        bool skip = (pc.flags & REFLECT_FLAG_ROUGH) != 0 && material_rough(material) > 0.0
            && any((px & 1) != 0);
        if (material_shine(material) > 0.0 && !skip)
        {
            float2 screen = float2((float(px.x) + 0.5) / float(w) * 2.0 - 1.0, 1.0 - (float(px.y) + 0.5) / float(h) * 2.0);
            Ray ray = screen_ray(screen);
            Ray next = screen_ray(screen + float2(2.0 / float(w), 0.0));
            float spread = length(next.dir - ray.dir);
            float3 n = oct_decode(g.zw);
            float3 p = ray.origin + ray.dir * g.x;
            float3 dir = reflect(ray.dir, n);
            result = reflected(p, n, dir, g.x, spread, px);
        }
    }
    images[pc.target][id.xy] = result;
}

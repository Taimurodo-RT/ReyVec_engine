// Сцена «3D», проход «тени 3D» (этап 7.4): по буферу геометрии (raymarch.slang) для
// каждой точки экрана считает, сколько света до неё доходит. Тени мягкие: луч от точки
// к свету идёт по тому же полю расстояний, и если он прошёл близко к форме, точка в
// полутени (приём Иниго Килеса, «soft shadows» по SDF). Затенение в углах — несколько
// проб поля вдоль нормали: чем ближе к ним другие формы, тем темнее угол.
//
// Точка результата (RGBA16F): x — солнце, y — затенение в углах (1 — открыто),
// z и w — две первые лампы (остальные светят без теней).

#include "include/space.slang"

[[vk::binding(0, 0)]] Texture2D textures[];

// Доля света от источника по направлению `dir` с расстояния `far`: 1 — свет виден
// целиком, 0 — закрыт. `k` — жёсткость края (больше — резче).
float soft_shadow(float3 from, float3 dir, float far, float k)
{
    float light = 1.0;
    float t = 0.02;
    float last = 1e9;
    for (int i = 0; i < 64; i++)
    {
        if (t >= far)
            break;
        float h = scene(from + dir * t).x;
        // Уточнение по двум последним шагам: меньше полос в полутени.
        float y = h * h / (2.0 * last);
        float d = sqrt(max(h * h - y * y, 0.0));
        light = min(light, k * d / max(t - y, 1e-4));
        last = h;
        if (light < 0.002)
            break;
        t += clamp(h, 0.02, 0.6);
    }
    light = saturate(light);
    return light * light * (3.0 - 2.0 * light);
}

// Затенение в углах: 1 — вокруг пусто, меньше — рядом другие формы.
float ambient_occlusion(float3 p, float3 n)
{
    float occlusion = 0.0;
    float weight = 1.0;
    for (uint i = 0; i < 5; i++)
    {
        float h = 0.02 + 0.12 * float(i);
        float d = scene(p + n * h).x;
        occlusion += (h - d) * weight;
        weight *= 0.85;
    }
    return saturate(1.0 - 2.5 * occlusion);
}

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    Surface s = read_surface(textures[pc.gbuffer].Load(int3(int2(input.position.xy), 0)));
    if (s.t < 0.0)
        return float4(1.0, 1.0, 1.0, 1.0);
    Ray ray = screen_ray(input.screen);
    // Чуть над поверхностью: иначе луч тени сразу «упирается» в саму точку.
    float3 p = ray.origin + ray.dir * s.t + s.normal * 0.004;

    float4 result = float4(1.0, ambient_occlusion(p, s.normal), 1.0, 1.0);
    if (dot(s.normal, SUN) > 0.0)
        result.x = soft_shadow(p, SUN, 30.0, 12.0);
    uint lamps = min(lamp_count(), SHADOW_LAMPS);
    for (uint i = 0; i < lamps; i++)
    {
        Lamp l = read_lamp(i);
        float3 to = l.at - p;
        float distance = length(to);
        float shadow = 1.0;
        // Только там, куда лампа светит. Луч останавливается, не дойдя до лампы на
        // 15 см: лампочка-шарик вокруг лампы не должна закрывать свою же лампу.
        if (distance < l.range && dot(s.normal, to) > 0.0)
            shadow = soft_shadow(p, to / distance, distance - 0.15, 10.0);
        if (i == 0)
            result.z = shadow;
        else
            result.w = shadow;
    }
    return result;
}

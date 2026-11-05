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
        // Граница объекта вместо формы, только если она так далеко, что полутени не даёт:
        // k · h / t заметно больше 1 (см. scene_near).
        float h = scene_near(from + dir * t, max(0.15, 2.0 * t / k)).x;
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
        float d = scene_near(p + n * h, 1e9).x;
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
    // Над поверхностью: иначе луч тени сразу «упирается» в саму точку, и по формам идёт
    // рябь. Шагание останавливается с точностью 0,5 мм на метр пути, поэтому дальние
    // точки поднимаются выше.
    float3 p = ray.origin + ray.dir * s.t + s.normal * (0.01 + 0.002 * s.t);

    // Солнце и две первые лампы перебираются одним циклом: луч тени (с полем сцены
    // внутри) встраивается в шейдер один раз. Иначе шейдер раздувается втрое, а
    // программный lavapipe на таком падал при сборке.
    float shadow[3] = { 1.0, 1.0, 1.0 };
    uint lamps = min(lamp_count(), SHADOW_LAMPS);
    for (uint k = 0; k <= lamps; k++)
    {
        float3 dir = SUN;
        float far = 30.0;
        float k_hard = 12.0;
        bool lit = true;
        if (k > 0)
        {
            Lamp l = read_lamp(k - 1);
            float3 to = l.at - p;
            float distance = length(to);
            dir = to / max(distance, 1e-4);
            // Луч останавливается, не дойдя до лампы 35 см: лампочка-шарик вокруг лампы
            // не должна закрывать свою же лампу (а луч, прошедший рядом с ней, — давать
            // полутень кольцами).
            far = distance - 0.35;
            k_hard = 10.0;
            lit = distance < l.range;
        }
        // Только там, куда свет падает: обратная сторона и так тёмная.
        if (lit && dot(s.normal, dir) > 0.0 && far > 0.0)
            shadow[k] = soft_shadow(p, dir, far, k_hard);
    }
    return float4(shadow[0], ambient_occlusion(p, s.normal), shadow[1], shadow[2]);
}

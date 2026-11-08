// Сцена «3D», проход «тени 3D» (этап 7.4): по буферу геометрии (raymarch.slang) для
// каждой точки экрана считает, сколько света до неё доходит. Тени мягкие: луч от точки
// к свету идёт по тому же полю расстояний, и если он прошёл близко к форме, точка в
// полутени (приём Иниго Килеса, «soft shadows» по SDF). Затенение в углах — несколько
// проб поля вдоль нормали: чем ближе к ним другие формы, тем темнее угол.
//
// Точка результата (RGBA16F): x — солнце, y — затенение в углах (1 — открыто),
// z и w — две первые лампы (остальные светят без теней).
//
// Результат в половинном разрешении (7.6б): точка (x, y) считается для пикселя кадра
// (2x, 2y) — левого верхнего в квадрате 2×2. Растягивает обратно raymarch_shade.slang.

#include "include/space.slang"
#include "include/shadow3d.slang"

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    uint w, h;
    textures[pc.gbuffer].GetDimensions(w, h);
    int2 px = min(int2(input.position.xy) * 2, int2(w, h) - 1);
    Surface s = read_surface(textures[pc.gbuffer].Load(int3(px, 0)));
    if (s.t < 0.0)
        return float4(1.0, 1.0, 1.0, 1.0);
    // Луч того же пикселя кадра, что и точка буфера геометрии.
    float2 screen = float2((float(px.x) + 0.5) / float(w) * 2.0 - 1.0, 1.0 - (float(px.y) + 0.5) / float(h) * 2.0);
    Ray ray = screen_ray(screen);
    float3 surface = ray.origin + ray.dir * s.t;
    // Солнце, углы и две первые лампы перебираются одним циклом: луч тени (с полем сцены
    // внутри) встраивается в шейдер один раз. Иначе шейдер раздувается втрое, а
    // программный lavapipe на таком падал при сборке.
    float shadow[4] = { 1.0, 1.0, 1.0, 1.0 };
    for (uint k = 0; k < 4; k++)
    {
        shadow[k] = shadow_part(surface, s.normal, s.t, k);
#ifdef SPACE_COUNTERS
        // Счётчики (7.18д2): затенение в углах — своей строкой.
        counters_flush_fragment(k == 1 ? PASS_AO : PASS_SHADOWS, px);
#endif
    }
    return float4(shadow[0], shadow[1], shadow[2], shadow[3]);
}

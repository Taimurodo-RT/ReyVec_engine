// Сцена «3D» в уменьшенном разрешении (7.14): растягивает освещённую картинку
// (raymarch_shade.slang в проходе «освещение 3D») на весь кадр в проходе «сцена».
//
// Только по одному кадру, без накопления по времени: каждая точка кадра берёт 4×4 точки
// маленькой картинки вокруг себя с весами Катмулла — Рома (кубическая кривая, резче
// простого смешивания соседей). Такая кривая на резком перепаде даёт светлую или тёмную
// кайму, поэтому итог зажат между самым тёмным и самым светлым из четырёх ближайших.
// Картинка — pc.target, размер кадра — pc.height и pc.aspect (space_push в demo.rs).

#include "include/space.slang"

[[vk::binding(0, 0)]] Texture2D textures[];

// Веса Катмулла — Рома для четырёх точек при доле t между второй и третьей.
float4 catmull_rom(float t)
{
    float t2 = t * t;
    float t3 = t2 * t;
    return float4(
        -0.5 * t3 + t2 - 0.5 * t,
        1.5 * t3 - 2.5 * t2 + 1.0,
        -1.5 * t3 + 2.0 * t2 + 0.5 * t,
        0.5 * t3 - 0.5 * t2);
}

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    Texture2D small = textures[pc.target];
    uint w, h;
    small.GetDimensions(w, h);
    int2 size = int2(w, h);
    float2 full = float2(pc.height * pc.aspect, pc.height);
    // Центр пикселя кадра в точках маленькой картинки (их центры на .5).
    float2 at = input.position.xy * float2(size) / full - 0.5;
    int2 base = int2(floor(at));
    float2 f = at - float2(base);
    float4 wx = catmull_rom(f.x);
    float4 wy = catmull_rom(f.y);
    float3 sum = 0.0;
    float3 lo = 1e30;
    float3 hi = -1e30;
    for (int j = 0; j < 4; j++)
    {
        float3 row = 0.0;
        for (int i = 0; i < 4; i++)
        {
            int2 q = clamp(base + int2(i - 1, j - 1), int2(0, 0), size - 1);
            float3 c = small.Load(int3(q, 0)).rgb;
            row += wx[i] * c;
            if ((i == 1 || i == 2) && (j == 1 || j == 2))
            {
                lo = min(lo, c);
                hi = max(hi, c);
            }
        }
        sum += wy[j] * row;
    }
    return float4(clamp(sum, lo, hi), 1.0);
}

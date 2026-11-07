// Сцена «3D» в уменьшенном разрешении (7.14): растягивает освещённую картинку
// (raymarch_shade.slang в проходе «освещение 3D») на весь кадр в проходе «сцена».
//
// Только по одному кадру, без накопления по времени: кубическая кривая Катмулла — Рома
// по 4×4 точкам маленькой картинки (резче простого смешивания соседей). Чтобы не читать
// 16 точек, она собрана из пяти чтений со сглаживающим сэмплером (7.15: так делают в
// играх; угловые четыре точки с малым весом отброшены). Такая кривая на резком перепаде
// даёт светлую или тёмную кайму, поэтому итог зажат между самым тёмным и самым светлым
// из четырёх ближайших точек.
// Картинка — pc.target, размер кадра — pc.height и pc.aspect (space_push в demo.rs).

#include "include/space.slang"

[[vk::binding(0, 0)]] Texture2D textures[];

[[vk::binding(1, 0)]] SamplerState samplers[2];

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    Texture2D small = textures[pc.target];
    uint w, h;
    small.GetDimensions(w, h);
    float2 size = float2(w, h);
    float2 full = float2(pc.height * pc.aspect, pc.height);
    // Центр пикселя кадра в точках маленькой картинки.
    float2 at = input.position.xy * size / full;
    float2 center = floor(at - 0.5) + 0.5; // центр точки слева сверху от него
    float2 f = at - center;
    float2 w0 = f * (-0.5 + f * (1.0 - 0.5 * f));
    float2 w1 = 1.0 + f * f * (-2.5 + 1.5 * f);
    float2 w2 = f * (0.5 + f * (2.0 - 1.5 * f));
    float2 w3 = f * f * (-0.5 + 0.5 * f);
    // Две средние точки — одним сглаженным чтением между ними.
    float2 w12 = w1 + w2;
    float2 t0 = (center - 1.0) / size;
    float2 t3 = (center + 2.0) / size;
    float2 t12 = (center + w2 / w12) / size;
    SamplerState s = samplers[0];
    float3 sum = small.SampleLevel(s, float2(t12.x, t0.y), 0.0).rgb * (w12.x * w0.y)
        + small.SampleLevel(s, float2(t0.x, t12.y), 0.0).rgb * (w0.x * w12.y)
        + small.SampleLevel(s, t12, 0.0).rgb * (w12.x * w12.y)
        + small.SampleLevel(s, float2(t3.x, t12.y), 0.0).rgb * (w3.x * w12.y)
        + small.SampleLevel(s, float2(t12.x, t3.y), 0.0).rgb * (w12.x * w3.y);
    float weight = w12.x * w0.y + w0.x * w12.y + w12.x * w12.y + w3.x * w12.y + w12.x * w3.y;
    sum /= weight;
    // Четыре ближайшие точки: пределы против каймы.
    int2 base = int2(center - 0.5);
    int2 last = int2(w, h) - 1;
    float3 lo = 1e30;
    float3 hi = -1e30;
    for (int k = 0; k < 4; k++)
    {
        int2 q = clamp(base + int2(k & 1, k >> 1), int2(0, 0), last);
        float3 c = small.Load(int3(q, 0)).rgb;
        lo = min(lo, c);
        hi = max(hi, c);
    }
    return float4(clamp(sum, lo, hi), 1.0);
}

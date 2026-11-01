// Jump flooding, один шаг: из 9 точек через step пикселей берётся ближайшая точка края.
// Шаги идут от половины размера картинки до 1 (и ещё раз 1). Точка края хранится
// сдвигом от своей точки маски (см. jfa_seed.slang): рядом с краем сдвиг маленький, и
// половинной точности картинки хватает с запасом.

#include "include/jfa.slang"

struct Push
{
    uint seeds; // номер картинки семян прошлого шага
    int step;
};
[[vk::push_constant]] ConstantBuffer<Push> pc;

[[vk::binding(0, 0)]] Texture2D textures[];

struct VertexOut
{
    float4 position : SV_Position;
};

[shader("vertex")]
VertexOut vs_main(uint vertex : SV_VulkanVertexID)
{
    // Вершины (-1,-1), (3,-1), (-1,3): треугольник накрывает весь экран.
    float2 uv = float2((vertex << 1) & 2, vertex & 2);
    VertexOut o;
    o.position = float4(uv * 2.0 - 1.0, 0.0, 1.0);
    return o;
}

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    uint w, h;
    textures[pc.seeds].GetDimensions(w, h);
    int2 size = int2(w, h);
    int2 q = int2(input.position.xy);
    float2 best = float2(NO_SEED, NO_SEED);
    float best_d = 1e30;
    for (int dy = -1; dy <= 1; dy++)
    {
        for (int dx = -1; dx <= 1; dx++)
        {
            int2 n = q + int2(dx, dy) * pc.step;
            if (any(n < int2(0, 0)) || any(n >= size))
                continue;
            float2 offset = textures[pc.seeds].Load(int3(n, 0)).xy;
            if (!has_seed(offset))
                continue;
            // Точка края соседа, теперь сдвигом от q.
            float2 v = float2(n - q) + offset;
            float d = dot(v, v);
            if (d < best_d)
            {
                best_d = d;
                best = v;
            }
        }
    }
    return float4(best, 0.0, 1.0);
}

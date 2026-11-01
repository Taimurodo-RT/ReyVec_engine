// Jump flooding, шаг 0: семена. Точка маски на краю (закрытая фигурой не целиком или с
// соседом по стороне по другую сторону половины) запоминает точку края рядом с собой —
// сдвигом от своей середины, в точках маски. Остальные — «нет семени» (NO_SEED).
// Маска хранит долю точки, закрытую фигурой, поэтому край ставится точнее размера точки,
// и линии поля не идут ступеньками (узкое место света 4).
// Зеркало на процессоре: reyvec_sdf::jump_flood и reyvec_sdf::edge_point.

#include "include/jfa.slang"

struct Push
{
    uint mask; // номер картинки маски
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

float coverage_at(int2 q, int2 size)
{
    q = clamp(q, int2(0, 0), size - 1);
    return textures[pc.mask].Load(int3(q, 0)).r;
}

// Точка края рядом с точкой маски s. Если s закрыта целиком или пуста, край проходит через
// соседа по стороне с долей ближе всего к половине. Доля c у точки края значит, что край
// на 0,5 − c от её середины наружу, против наклона доли (наклон по 8 соседям,
// диагональные с меньшим весом).
float2 edge_point(int2 s, int2 size)
{
    int2 t = s;
    float c = coverage_at(s, size);
    if (c <= 0.0 || c >= 1.0)
    {
        int2 around[4] = { int2(1, 0), int2(-1, 0), int2(0, 1), int2(0, -1) };
        for (int i = 0; i < 4; i++)
        {
            int2 n = s + around[i];
            if (abs(coverage_at(n, size) - 0.5) < abs(coverage_at(t, size) - 0.5))
                t = n;
        }
        c = coverage_at(t, size);
    }
    const float r2 = 1.41421356;
    float gx = coverage_at(t + int2(1, -1), size) + r2 * coverage_at(t + int2(1, 0), size)
        + coverage_at(t + int2(1, 1), size) - coverage_at(t + int2(-1, -1), size)
        - r2 * coverage_at(t + int2(-1, 0), size) - coverage_at(t + int2(-1, 1), size);
    float gy = coverage_at(t + int2(-1, 1), size) + r2 * coverage_at(t + int2(0, 1), size)
        + coverage_at(t + int2(1, 1), size) - coverage_at(t + int2(-1, -1), size)
        - r2 * coverage_at(t + int2(0, -1), size) - coverage_at(t + int2(1, -1), size);
    float2 slope = float2(gx, gy);
    float len = length(slope);
    if (len < 1e-4)
        return float2(t);
    return float2(t) - slope * ((c - 0.5) / len);
}

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    uint w, h;
    textures[pc.mask].GetDimensions(w, h);
    int2 size = int2(w, h);
    int2 q = int2(input.position.xy);
    float c = coverage_at(q, size);
    bool m = c >= 0.5;
    bool edge = (c > 0.0 && c < 1.0)
        || (coverage_at(q + int2(1, 0), size) >= 0.5) != m
        || (coverage_at(q - int2(1, 0), size) >= 0.5) != m
        || (coverage_at(q + int2(0, 1), size) >= 0.5) != m
        || (coverage_at(q - int2(0, 1), size) >= 0.5) != m;
    // У края картинки сосед за краем — та же точка (clamp), он не делает её семенем.
    float2 offset = edge ? edge_point(q, size) - float2(q) : float2(NO_SEED, NO_SEED);
    return float4(offset, 0.0, 1.0);
}

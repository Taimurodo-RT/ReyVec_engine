// Способ 2, проход трафарета: считает, сколько раз контур фигуры оборачивается вокруг
// каждой точки пикселя. Цвет не пишет (см. `curves.rs` в reyvec-vector).
//
// Треугольники веера всегда целиком внутри. У треугольника кривой точка внутри, если
// u² − v ≤ 0. Проверка идёт для каждой точки MSAA отдельно: так край кривой сглаживается
// так же, как прямые края, а не ступеньками по целым пикселям.

struct CurveVertex
{
    float2 position; // в координатах фигуры
    float2 uv;       // координаты кривой: (0,0), (½,0), (1,1); у веера (0,1)
};

struct Instance
{
    float4 linear;
    float2 offset;
    uint first_index;       // треугольники способа 1 (здесь не нужны)
    uint curve_first_index; // где начинаются треугольники веера и кривых фигуры
    float4 color;
};

struct Push
{
    uint* indices;
    CurveVertex* vertices;
    Instance* instances;
    float aspect;
    uint samples; // точек MSAA на пиксель: 1, 4 или 8
};
[[vk::push_constant]] ConstantBuffer<Push> pc;

struct VertexOut
{
    float4 position : SV_Position;
    float2 uv : CURVE;
};

[shader("vertex")]
VertexOut vs_main(uint vertex : SV_VulkanVertexID, uint instance : SV_VulkanInstanceID)
{
    Instance inst = pc.instances[instance];
    CurveVertex v = pc.vertices[pc.indices[inst.curve_first_index + vertex]];
    float2 p = float2(
        inst.linear.x * v.position.x + inst.linear.z * v.position.y,
        inst.linear.y * v.position.x + inst.linear.w * v.position.y) + inst.offset;
    p.x /= pc.aspect;
    VertexOut o;
    o.position = float4(p.x, -p.y, 0.0, 1.0);
    o.uv = v.uv;
    return o;
}

// Стандартные положения точек MSAA в Vulkan (доли пикселя от левого верхнего угла).
static const float2 SAMPLES_4[4] = {
    float2(0.375, 0.125), float2(0.875, 0.375), float2(0.125, 0.625), float2(0.625, 0.875)
};
static const float2 SAMPLES_8[8] = {
    float2(0.5625, 0.3125), float2(0.4375, 0.6875), float2(0.8125, 0.5625), float2(0.3125, 0.1875),
    float2(0.1875, 0.8125), float2(0.0625, 0.4375), float2(0.6875, 0.9375), float2(0.9375, 0.0625)
};

bool inside(float2 uv)
{
    return uv.x * uv.x - uv.y <= 0.0;
}

[shader("fragment")]
float4 fs_main(VertexOut input, out uint coverage : SV_Coverage) : SV_Target
{
    // uv меняется по треугольнику линейно, поэтому в любой точке пикселя его можно
    // получить из значения в центре и изменения к соседним пикселям.
    float2 dx = ddx(input.uv);
    float2 dy = ddy(input.uv);
    uint mask = 0;
    if (pc.samples >= 8)
    {
        for (uint i = 0; i < 8; i++)
        {
            float2 o = SAMPLES_8[i] - 0.5;
            if (inside(input.uv + dx * o.x + dy * o.y)) mask |= 1u << i;
        }
    }
    else if (pc.samples >= 4)
    {
        for (uint i = 0; i < 4; i++)
        {
            float2 o = SAMPLES_4[i] - 0.5;
            if (inside(input.uv + dx * o.x + dy * o.y)) mask |= 1u << i;
        }
    }
    else if (inside(input.uv))
    {
        mask = 1;
    }
    // Маска 0 ничего не пишет: это и есть «отбросить пиксель» (без discard, которому
    // нужна отдельная возможность видеокарты).
    coverage = mask;
    return float4(0.0, 0.0, 0.0, 0.0);
}

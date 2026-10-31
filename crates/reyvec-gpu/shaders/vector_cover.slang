// Способ 2, проход заливки: красит пиксели фигуры, где трафарет не ноль, и сразу
// обнуляет трафарет (это делает настройка конвейера, а не шейдер). Рисует те же
// треугольники веера и кривых, что и проход трафарета: вместе они накрывают всю фигуру.

struct CurveVertex
{
    float2 position;
    float2 uv;
};

struct Instance
{
    float4 linear;
    float2 offset;
    uint first_index;
    uint curve_first_index;
    float4 color;
};

struct Push
{
    uint* indices;
    CurveVertex* vertices;
    Instance* instances;
    float aspect;
    uint samples;
};
[[vk::push_constant]] ConstantBuffer<Push> pc;

struct VertexOut
{
    float4 position : SV_Position;
    nointerpolation float4 color : COLOR;
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
    o.color = inst.color;
    return o;
}

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    return input.color;
}

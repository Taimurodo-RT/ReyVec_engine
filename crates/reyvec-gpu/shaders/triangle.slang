// Тестовый треугольник этапа 1. Вершины зашиты в шейдер, буферов нет.
// Соглашение движка: Y смотрит вверх, поэтому для Vulkan Y переворачиваем.

struct Push
{
    float time;   // секунды с запуска
    float aspect; // ширина / высота, чтобы треугольник не растягивался
};
[[vk::push_constant]] ConstantBuffer<Push> pc;

struct VertexOut
{
    float4 position : SV_Position;
    float3 color : COLOR;
};

static const float2 POSITIONS[3] = { float2(0.0, 0.6), float2(-0.52, -0.3), float2(0.52, -0.3) };
static const float3 COLORS[3] = { float3(1.0, 0.25, 0.2), float3(0.2, 1.0, 0.35), float3(0.25, 0.4, 1.0) };

[shader("vertex")]
VertexOut vs_main(uint vertex : SV_VulkanVertexID)
{
    float c = cos(pc.time * 0.5);
    float s = sin(pc.time * 0.5);
    float2 q = POSITIONS[vertex];
    float2 p = float2(c * q.x - s * q.y, s * q.x + c * q.y);
    p.x /= pc.aspect;

    VertexOut o;
    o.position = float4(p.x, -p.y, 0.0, 1.0);
    o.color = COLORS[vertex];
    return o;
}

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    return float4(input.color, 1.0);
}

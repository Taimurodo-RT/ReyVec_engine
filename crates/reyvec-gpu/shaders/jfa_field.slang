// Jump flooding, итог: расстояние со знаком до границы маски в единицах сцены
// (внутри отрицательное). Где фигур нет вовсе — большое число.

struct Push
{
    uint seeds;          // номер картинки семян после последнего шага
    uint mask;           // номер картинки маски (знак)
    float scene_per_pixel; // сколько единиц сцены в одном пикселе маски
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
    int2 q = int2(input.position.xy);
    float2 s = textures[pc.seeds].Load(int3(q, 0)).xy;
    if (s.x < 0.0)
        return float4(10000.0, 0.0, 0.0, 1.0);
    float d = (length(s - float2(q)) + 0.5) * pc.scene_per_pixel;
    bool inside = textures[pc.mask].Load(int3(q, 0)).r > 0.5;
    return float4(inside ? -d : d, 0.0, 0.0, 1.0);
}

// Jump flooding, шаг 0: семена. Пиксель на границе маски (сосед по стороне с другим
// значением) запоминает свои координаты, остальные — «нет семени» (-1, -1).
// Зеркало на процессоре: reyvec_sdf::jump_flood.

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

bool inside_at(int2 q, int2 size)
{
    q = clamp(q, int2(0, 0), size - 1);
    return textures[pc.mask].Load(int3(q, 0)).r > 0.5;
}

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    uint w, h;
    textures[pc.mask].GetDimensions(w, h);
    int2 size = int2(w, h);
    int2 q = int2(input.position.xy);
    bool m = inside_at(q, size);
    bool edge = inside_at(q + int2(1, 0), size) != m || inside_at(q - int2(1, 0), size) != m
        || inside_at(q + int2(0, 1), size) != m || inside_at(q - int2(0, 1), size) != m;
    float2 seed = edge ? float2(q) : float2(-1.0, -1.0);
    return float4(seed, 0.0, 1.0);
}

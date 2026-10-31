// Панель egui. Вершины и индексы шейдер читает из буфера по адресу, текстуру берёт
// из общего списка (bindless). Цвета egui хранит в sRGB с умноженной альфой.

struct Push
{
    uint* indices;   // номера вершин, по три на треугольник
    uint* vertices;  // по 5 слов на вершину: x, y, u, v, цвет (RGBA по 8 бит)
    float2 screen;   // размер экрана в точках egui
    uint texture;    // номер в textures[]
    uint sampler_id; // 0 = со сглаживанием, 1 = чёткие пиксели
    uint base_vertex;
};
[[vk::push_constant]] ConstantBuffer<Push> pc;

[[vk::binding(0, 0)]] Texture2D textures[];
[[vk::binding(1, 0)]] SamplerState samplers[2];

struct VertexOut
{
    float4 position : SV_Position;
    float2 uv : UV;
    float4 color : COLOR;
};

float3 srgb_to_linear(float3 c)
{
    float3 lo = c / 12.92;
    float3 hi = pow((c + 0.055) / 1.055, 2.4);
    return select(c <= 0.04045, lo, hi);
}

[shader("vertex")]
VertexOut vs_main(uint vertex : SV_VulkanVertexID)
{
    uint i = (pc.indices[vertex] + pc.base_vertex) * 5;
    float2 pos = float2(asfloat(pc.vertices[i]), asfloat(pc.vertices[i + 1]));
    float2 uv = float2(asfloat(pc.vertices[i + 2]), asfloat(pc.vertices[i + 3]));
    uint packed = pc.vertices[i + 4];
    float4 c = float4(packed & 0xff, (packed >> 8) & 0xff, (packed >> 16) & 0xff, packed >> 24) / 255.0;

    VertexOut o;
    // Точки egui: (0, 0) в левом верхнем углу, y вниз. Как и у Vulkan, поэтому без переворота.
    o.position = float4(pos / pc.screen * 2.0 - 1.0, 0.0, 1.0);
    o.uv = uv;
    // Окно в sRGB: видеокарта сама переводит линейный цвет в цвет экрана.
    o.color = float4(srgb_to_linear(c.rgb), c.a);
    return o;
}

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    Texture2D t = textures[NonUniformResourceIndex(pc.texture)];
    float4 tex = pc.sampler_id == 1 ? t.Sample(samplers[1], input.uv) : t.Sample(samplers[0], input.uv);
    return input.color * tex;
}

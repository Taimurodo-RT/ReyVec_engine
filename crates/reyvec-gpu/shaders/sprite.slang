// Спрайты с текстурами. Данные спрайтов шейдер читает из буфера по адресу
// (buffer device address), текстуры берёт из общего списка (bindless).

struct Sprite
{
    float2 pos;      // центр: y от -1 до 1, x от -aspect до aspect
    float2 size;     // ширина и высота в тех же единицах
    float4 tint;     // множитель цвета, линейный
    float rotation;  // радианы, против часовой стрелки
    uint texture;    // номер в textures[]
    uint sampler_id; // 0 = со сглаживанием, 1 = чёткие пиксели
    uint pad;
};

struct Push
{
    Sprite* sprites;
    float aspect;
};
[[vk::push_constant]] ConstantBuffer<Push> pc;

[[vk::binding(0, 0)]] Texture2D textures[];
[[vk::binding(1, 0)]] SamplerState samplers[2];

struct VertexOut
{
    float4 position : SV_Position;
    float2 uv : UV;
    float4 tint : TINT;
    nointerpolation uint texture : TEXTURE;
    nointerpolation uint sampler_id : SAMPLER;
};

static const float2 CORNERS[6] = {
    float2(-0.5, -0.5), float2(0.5, -0.5), float2(0.5, 0.5),
    float2(-0.5, -0.5), float2(0.5, 0.5), float2(-0.5, 0.5)
};

[shader("vertex")]
VertexOut vs_main(uint vertex : SV_VulkanVertexID, uint instance : SV_VulkanInstanceID)
{
    Sprite s = pc.sprites[instance];
    float2 corner = CORNERS[vertex];
    float c = cos(s.rotation);
    float n = sin(s.rotation);
    float2 local = corner * s.size;
    float2 p = s.pos + float2(c * local.x - n * local.y, n * local.x + c * local.y);
    p.x /= pc.aspect;

    VertexOut o;
    o.position = float4(p.x, -p.y, 0.0, 1.0);
    o.uv = float2(corner.x + 0.5, 0.5 - corner.y);
    o.tint = s.tint;
    o.texture = s.texture;
    o.sampler_id = s.sampler_id;
    return o;
}

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    Texture2D t = textures[NonUniformResourceIndex(input.texture)];
    float4 c = input.sampler_id == 1 ? t.Sample(samplers[1], input.uv) : t.Sample(samplers[0], input.uv);
    return c * input.tint;
}

// Опыт этапа 4 (задача 4.6): свет из каскадов лучей (rc_cascade.slang). Каждая точка
// картинки света берёт четыре ближайших зонда каскада 0 и усредняет их лучи по всем
// направлениям: это и есть свет, пришедший в точку со всех сторон. Картинка та же, что
// у обычного прохода «свет» (light.slang), поэтому вывод и свечение не меняются.

struct Push
{
    uint cascade0;      // картинка каскада 0
    uint pad0;
    uint2 probes;       // зондов каскада 0 по x и y (по зонду на точку поля)
    float ambient_r;    // свет, который есть везде
    float ambient_g;
    float ambient_b;
    float pad1;
};
[[vk::push_constant]] ConstantBuffer<Push> pc;

[[vk::binding(0, 0)]] Texture2D textures[];

struct VertexOut
{
    float4 position : SV_Position;
    float2 uv : UV;
};

[shader("vertex")]
VertexOut vs_main(uint vertex : SV_VulkanVertexID)
{
    float2 uv = float2((vertex << 1) & 2, vertex & 2);
    VertexOut o;
    o.position = float4(uv * 2.0 - 1.0, 0.0, 1.0);
    o.uv = uv;
    return o;
}

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    // Зонд каскада 0 стоит в середине каждой точки поля.
    float2 q = input.uv * float2(pc.probes) - 0.5;
    int2 base = int2(floor(q));
    float2 k = q - float2(base);
    float3 sum = float3(0.0, 0.0, 0.0);
    for (int y = 0; y < 2; y++)
    {
        for (int x = 0; x < 2; x++)
        {
            int2 probe = clamp(base + int2(x, y), int2(0, 0), int2(pc.probes) - 1);
            float w = (x == 0 ? 1.0 - k.x : k.x) * (y == 0 ? 1.0 - k.y : k.y);
            float3 rays = float3(0.0, 0.0, 0.0);
            for (int c = 0; c < 4; c++)
            {
                int2 at = probe * 2 + int2(c % 2, c / 2);
                rays += textures[pc.cascade0].Load(int3(at, 0)).rgb;
            }
            sum += rays * 0.25 * w;
        }
    }
    return float4(sum + float3(pc.ambient_r, pc.ambient_g, pc.ambient_b), 0.0);
}

// Свечение (bloom, задача 4.4), шаг вверх: картинка вдвое меньше растягивается мягким
// фильтром 3×3 («палатка») и складывается с картинкой этого размера из шага вниз
// (bloom_down.slang). Каждая ступень цепочки добавляет свой размер размытия: вблизи
// ярких мест свечение плотное, дальше широкое и слабое.

struct Push
{
    uint small;  // собранная картинка ступенью меньше
    uint same;   // картинка шага вниз этого размера
    float2 texel; // размер точки цели в долях картинки (1 / размер)
};
[[vk::push_constant]] ConstantBuffer<Push> pc;

[[vk::binding(0, 0)]] Texture2D textures[];
[[vk::binding(1, 0)]] SamplerState samplers[2];

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

float3 small_at(float2 uv)
{
    return textures[pc.small].SampleLevel(samplers[0], uv, 0.0).rgb;
}

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    float2 uv = input.position.xy * pc.texel;
    float2 s = pc.texel;
    float3 sum = small_at(uv) * 4.0;
    sum += (small_at(uv + float2(s.x, 0.0)) + small_at(uv - float2(s.x, 0.0))
        + small_at(uv + float2(0.0, s.y)) + small_at(uv - float2(0.0, s.y))) * 2.0;
    sum += small_at(uv + s) + small_at(uv - s) + small_at(uv + float2(s.x, -s.y))
        + small_at(uv + float2(-s.x, s.y));
    float3 same = textures[pc.same].Load(int3(int2(input.position.xy), 0)).rgb;
    return float4(same + sum / 16.0, 1.0);
}

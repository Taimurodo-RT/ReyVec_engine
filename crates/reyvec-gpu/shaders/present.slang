// Вывод на экран: копирует готовый кадр сцены из временной картинки в окно.
// Один треугольник на весь экран, каждый пиксель читается из картинки как есть. В сцене
// «лампы и тени» (4.3) кадр умножается на свет (light.slang, картинка меньше кадра,
// значения между её точками смешиваются), а у самих ламп добавляется их свечение.
// Кадр сцены хранится в линейном цвете, перевод в цвет экрана делает формат окна (sRGB).

struct Push
{
    uint texture; // номер картинки сцены в общем списке текстур
    uint light;   // номер картинки света (если use_light)
    uint use_light;
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

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    float4 color = textures[pc.texture].Load(int3(int2(input.position.xy), 0));
    if (pc.use_light != 0)
    {
        uint w, h;
        textures[pc.texture].GetDimensions(w, h);
        float2 uv = input.position.xy / float2(w, h);
        float4 light = textures[pc.light].SampleLevel(samplers[0], uv, 0.0);
        color.rgb = color.rgb * light.rgb + light.rgb * light.a * 0.6;
    }
    return color;
}

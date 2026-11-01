// Вывод на экран: копирует готовый кадр сцены из временной картинки в окно.
// Один треугольник на весь экран, каждый пиксель читается из картинки как есть. В сцене
// «лампы и тени» (4.3) кадр умножается на свет (light.slang), а у самих ламп добавляется
// их свечение. Картинка света меньше кадра; при растяжении смешиваются только её точки с
// той же стороны края фигуры, что и пиксель (край виден по полю расстояний), иначе тень
// снаружи заползает на край фигуры ступеньками (узкое место света 5).
// Кадр сцены хранится в линейном цвете, перевод в цвет экрана делает формат окна (sRGB).

struct Push
{
    uint texture; // номер картинки сцены в общем списке текстур
    uint light;   // номер картинки света (если use_light)
    uint use_light;
    uint field;   // номер картинки поля расстояний (того же размера, что свет)
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

// Свет в точке uv: смесь четырёх ближайших точек картинки света, как у сэмплера, но
// точки по другую сторону края фигуры почти не берутся.
float4 light_at(float2 uv)
{
    uint lw, lh;
    textures[pc.light].GetDimensions(lw, lh);
    int2 size = int2(lw, lh);
    bool inside = textures[pc.field].SampleLevel(samplers[0], uv, 0.0).r < 0.0;
    float2 f = uv * float2(size) - 0.5;
    int2 base = int2(floor(f));
    float2 k = f - float2(base);
    float4 sum = float4(0.0, 0.0, 0.0, 0.0);
    float total = 0.0;
    for (int y = 0; y < 2; y++)
    {
        for (int x = 0; x < 2; x++)
        {
            int2 q = clamp(base + int2(x, y), int2(0, 0), size - 1);
            float wx = x == 0 ? 1.0 - k.x : k.x;
            float wy = y == 0 ? 1.0 - k.y : k.y;
            bool same = (textures[pc.field].Load(int3(q, 0)).r < 0.0) == inside;
            float weight = wx * wy * (same ? 1.0 : 0.001) + 1e-6;
            sum += textures[pc.light].Load(int3(q, 0)) * weight;
            total += weight;
        }
    }
    return sum / total;
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
        float4 light = light_at(uv);
        color.rgb = color.rgb * light.rgb + light.rgb * light.a * 0.6;
    }
    return color;
}

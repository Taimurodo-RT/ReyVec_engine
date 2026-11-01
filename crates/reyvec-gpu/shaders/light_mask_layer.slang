// Маска теней (задача 4.2б): силуэт слоя векторных фигур с отметкой «отбрасывает тень».
// Фигуры такого слоя уже нарисованы в его картинку (прозрачный фон), так что здесь они
// не рисуются заново: проход «маска» читает прозрачность картинки слоя (долю точки маски,
// закрытую фигурами) и добавляет её к маске. Смешивание «экран» объединяет её с тем, что
// уже нарисовал light_mask.slang: a + b − a·b. Один треугольник на всю маску, ножницы
// ограничивают его прямоугольником слоя.

struct Push
{
    uint texture;   // номер картинки слоя в общем списке текстур
    float downscale; // во сколько раз маска меньше кадра
    int2 offset;    // пиксель кадра, с которого начинается картинка слоя
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
    // Вершины (-1,-1), (3,-1), (-1,3): треугольник накрывает всю маску.
    float2 uv = float2((vertex << 1) & 2, vertex & 2);
    VertexOut o;
    o.position = float4(uv * 2.0 - 1.0, 0.0, 1.0);
    return o;
}

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    // Середина точки маски в пикселях кадра, затем в пикселях картинки слоя. Точка маски
    // накрывает downscale × downscale пикселей кадра: четыре выборки сэмплером (каждая
    // смешивает 2×2 пикселя) усредняют их все при уменьшении в 2 и 4 раза.
    float2 frame_pixel = input.position.xy * pc.downscale;
    uint w, h;
    textures[pc.texture].GetDimensions(w, h);
    float2 size = float2(w, h);
    float2 center = frame_pixel - float2(pc.offset);
    float q = pc.downscale * 0.25;
    float alpha = 0.0;
    alpha += textures[pc.texture].SampleLevel(samplers[0], (center + float2(-q, -q)) / size, 0.0).a;
    alpha += textures[pc.texture].SampleLevel(samplers[0], (center + float2(q, -q)) / size, 0.0).a;
    alpha += textures[pc.texture].SampleLevel(samplers[0], (center + float2(-q, q)) / size, 0.0).a;
    alpha += textures[pc.texture].SampleLevel(samplers[0], (center + float2(q, q)) / size, 0.0).a;
    return float4(alpha * 0.25, 0.0, 0.0, 1.0);
}

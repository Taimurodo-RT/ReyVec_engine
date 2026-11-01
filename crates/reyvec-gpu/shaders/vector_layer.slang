// Наложение готового слоя векторных фигур на то, что под ним.
// Один треугольник на весь экран читает картинку слоя (цвет уже умножен на прозрачность)
// и умножает её на прозрачность слоя. Режим смешивания (обычное, умножение, экран,
// сложение) задаёт смешивание конвейера, шейдер у всех режимов один.

struct Push
{
    uint texture;  // номер картинки слоя в общем списке текстур
    float opacity; // прозрачность слоя целиком
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
    return textures[pc.texture].Load(int3(int2(input.position.xy), 0)) * pc.opacity;
}

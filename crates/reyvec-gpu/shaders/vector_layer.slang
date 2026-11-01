// Наложение готового слоя векторных фигур на то, что под ним.
// Один треугольник на весь экран (ножницы ограничивают его прямоугольником слоя) читает
// картинку слоя (цвет уже умножен на прозрачность), умножает её на прозрачность слоя и,
// если у слоя есть маска, на маску. Режим смешивания (обычное, умножение, экран,
// сложение) задаёт смешивание конвейера, шейдер у всех режимов один.

struct Push
{
    uint texture;   // номер картинки слоя в общем списке текстур
    float opacity;  // прозрачность слоя целиком
    uint mask;      // номер картинки маски (если mask_mode не 0)
    uint mask_mode; // 0 нет маски, 1 по прозрачности, 2 по яркости, +4 обратная
    int2 offset;    // пиксель цели + offset = пиксель картинки слоя (она размером с
                    // прямоугольник слоя, а не с кадр)
    int2 mask_offset; // то же для картинки маски
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

float linear_to_srgb(float c)
{
    return c <= 0.0031308 ? c * 12.92 : 1.055 * pow(c, 1.0 / 2.4) - 0.055;
}

// Сколько слоя видно по маске `m` (цвет маски умножен на её прозрачность).
float mask_value(float4 m, uint mode)
{
    float v;
    if ((mode & 3) == 2)
    {
        // По яркости, как в SVG и браузерах: яркость считается по цвету в sRGB (без
        // умножения на прозрачность) и умножается на прозрачность.
        float3 c = m.a > 0.0 ? saturate(m.rgb / m.a) : float3(0.0, 0.0, 0.0);
        float3 s = float3(linear_to_srgb(c.r), linear_to_srgb(c.g), linear_to_srgb(c.b));
        v = dot(s, float3(0.2125, 0.7154, 0.0721)) * m.a;
    }
    else
    {
        v = m.a;
    }
    return (mode & 4) != 0 ? 1.0 - v : v;
}

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    int2 pixel = int2(input.position.xy);
    float k = pc.opacity;
    if (pc.mask_mode != 0)
        k *= mask_value(textures[pc.mask].Load(int3(pixel + pc.mask_offset, 0)), pc.mask_mode);
    return textures[pc.texture].Load(int3(pixel + pc.offset, 0)) * k;
}

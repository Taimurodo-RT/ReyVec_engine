// Сведение MSAA: из картинки с несколькими точками на пиксель делает обычный кадр сцены.
// Один треугольник на весь экран; каждый пиксель усредняет свои точки.
//
// Два способа усреднять:
// - «как в редакторах» (perceptual = 1): точки переводятся в цвет экрана (sRGB), там
//   усредняются и переводятся обратно. Так сглаживают Illustrator и браузеры: светлая
//   фигура на тёмном фоне не кажется толще и мягче, чем есть;
// - «физически точно» (perceptual = 0): точки усредняются в линейном цвете, как свет.

struct Push
{
    uint texture;    // номер картинки MSAA в общем списке текстур
    uint samples;    // точек на пиксель
    uint perceptual; // 1: усреднять в цвете экрана, 0: в линейном
};
[[vk::push_constant]] ConstantBuffer<Push> pc;

// Тот же общий список, что и Texture2D textures[] в других шейдерах: ячейка картинки
// MSAA хранит картинку с несколькими точками на пиксель.
[[vk::binding(0, 0)]] Texture2DMS<float4> textures_ms[];

struct VertexOut
{
    float4 position : SV_Position;
};

float3 linear_to_srgb(float3 c)
{
    c = max(c, 0.0);
    float3 lo = c * 12.92;
    float3 hi = 1.055 * pow(c, 1.0 / 2.4) - 0.055;
    return select(c <= 0.0031308, lo, hi);
}

float3 srgb_to_linear(float3 c)
{
    float3 lo = c / 12.92;
    float3 hi = pow((c + 0.055) / 1.055, 2.4);
    return select(c <= 0.04045, lo, hi);
}

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
    int2 p = int2(input.position.xy);
    Texture2DMS<float4> t = textures_ms[pc.texture];
    float4 sum = 0.0;
    for (uint i = 0; i < pc.samples; i++)
    {
        float4 c = t.Load(p, i);
        if (pc.perceptual != 0)
            c.rgb = linear_to_srgb(c.rgb);
        sum += c;
    }
    float4 avg = sum / float(pc.samples);
    if (pc.perceptual != 0)
        avg.rgb = srgb_to_linear(avg.rgb);
    return avg;
}

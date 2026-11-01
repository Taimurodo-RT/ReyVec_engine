// Вывод на экран: копирует готовый кадр сцены из временной картинки в окно.
// Один треугольник на весь экран, каждый пиксель читается из картинки как есть. В сцене
// «лампы и тени» (4.3) кадр умножается на свет (light.slang), а у самих ламп добавляется
// их свечение. Картинка света меньше кадра; при растяжении смешиваются только её точки с
// той же стороны края фигуры, что и пиксель (край виден по полю расстояний), иначе тень
// снаружи заползает на край фигуры ступеньками (узкое место света 5).
// Свечение и пересветы (4.4): к освещённому кадру добавляется свечение ярких мест
// (bloom_down/bloom_up.slang), а яркость выше белого мягко сводится к белому (тонмаппинг),
// вместо того чтобы обрезаться пятнами одного цвета. Лёгкий шум (дизеринг) убирает
// полосы на плавных переходах: экран хранит 256 ступеней яркости, и без шума плавный
// тёмный переход виден ступенями (узкое место векторов 53).
// Кадр сцены хранится в линейном цвете, перевод в цвет экрана делает формат окна (sRGB).

struct Push
{
    uint texture; // номер картинки сцены в общем списке текстур
    uint light;   // номер картинки света (если use_light)
    uint use_light;
    uint field;   // номер картинки поля расстояний (бывает меньше картинки света)
    uint bloom;   // номер картинки свечения (если флаг FLAG_GLOW)
    uint flags;   // FLAG_GLOW, FLAG_DITHER
    float glow;   // сила свечения
    float pad;
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
            float2 at = (float2(q) + 0.5) / float2(size);
            bool same = (textures[pc.field].SampleLevel(samplers[0], at, 0.0).r < 0.0) == inside;
            float weight = wx * wy * (same ? 1.0 : 0.001) + 1e-6;
            sum += textures[pc.light].Load(int3(q, 0)) * weight;
            total += weight;
        }
    }
    return sum / total;
}

static const uint FLAG_GLOW = 1;
static const uint FLAG_DITHER = 2;

// Мягкий переход к белому: до SHOULDER яркость как есть (цвета векторов не меняются),
// выше плавно приближается к 1 и никогда её не перескакивает. Сжимается самый яркий
// канал, остальные в той же пропорции, поэтому оттенок сохраняется; очень яркое
// (в несколько раз ярче белого) постепенно белеет, как пересвет на фото.
static const float SHOULDER = 0.8;

float3 tonemap(float3 c)
{
    float m = max(c.r, max(c.g, c.b));
    if (m <= SHOULDER)
        return c;
    float over = (m - SHOULDER) / (1.0 - SHOULDER);
    float mapped = SHOULDER + (1.0 - SHOULDER) * (1.0 - exp(-over));
    float3 hue = c * (mapped / m);
    float white = saturate((m - 1.0) / 4.0);
    return lerp(hue, float3(mapped, mapped, mapped), white * 0.6);
}

float srgb_encode(float x)
{
    return x <= 0.0031308 ? x * 12.92 : 1.055 * pow(x, 1.0 / 2.4) - 0.055;
}

float srgb_decode(float x)
{
    return x <= 0.04045 ? x / 12.92 : pow((x + 0.055) / 1.055, 2.4);
}

// Шум для пикселя p в пределах ±1 ступени экрана (сумма двух равномерных — треугольное
// распределение: так ступени исчезают, а шум почти не заметен). Не меняется от кадра к
// кадру, чтобы не мерцать.
float noise(float2 p)
{
    float a = frac(52.9829189 * frac(dot(p, float2(0.06711056, 0.00583715))));
    float b = frac(52.9829189 * frac(dot(p + float2(17.0, 59.0), float2(0.00583715, 0.06711056))));
    return a + b - 1.0;
}

float3 dither(float3 c, float2 p)
{
    float n = noise(p) / 255.0;
    float3 e = float3(srgb_encode(saturate(c.r)), srgb_encode(saturate(c.g)),
                      srgb_encode(saturate(c.b)));
    e = saturate(e + n);
    return float3(srgb_decode(e.r), srgb_decode(e.g), srgb_decode(e.b));
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
        if ((pc.flags & FLAG_GLOW) != 0)
        {
            float3 glow = textures[pc.bloom].SampleLevel(samplers[0], uv, 0.0).rgb;
            color.rgb = tonemap(color.rgb + glow * pc.glow);
        }
    }
    if ((pc.flags & FLAG_DITHER) != 0)
        color.rgb = dither(color.rgb, input.position.xy);
    return color;
}

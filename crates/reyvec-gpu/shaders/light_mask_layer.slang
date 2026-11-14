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
    float2 extent;  // размер прямоугольника слоя в пикселях: дальше в картинке не
                    // очищенная память (картинка больше прямоугольника), её не читаем
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

// Прозрачность картинки слоя в точке `pixel` (пиксели картинки), смесь 2×2 соседних
// пикселей по положению точки. За прямоугольником слоя память картинки не очищена
// (картинка больше прямоугольника), поэтому смесь считается вручную из пикселей,
// прижатых к прямоугольнику. Раньше точку прижимали на полпикселя и брали сэмплером, но
// у края сэмплер всё равно касается пикселя за прямоугольником с почти нулевым весом
// (веса округляются), и мусор оттуда — особенно не-число — попадал в маску (8.0).
float layer_alpha(float2 pixel)
{
    int2 last = int2(pc.extent) - 1;
    float2 p = clamp(pixel, float2(0.5, 0.5), pc.extent - 0.5) - 0.5;
    int2 i = int2(floor(p));
    float2 f = p - float2(i);
    float a00 = textures[pc.texture].Load(int3(clamp(i, int2(0, 0), last), 0)).a;
    float a10 = textures[pc.texture].Load(int3(clamp(i + int2(1, 0), int2(0, 0), last), 0)).a;
    float a01 = textures[pc.texture].Load(int3(clamp(i + int2(0, 1), int2(0, 0), last), 0)).a;
    float a11 = textures[pc.texture].Load(int3(clamp(i + int2(1, 1), int2(0, 0), last), 0)).a;
    return lerp(lerp(a00, a10, f.x), lerp(a01, a11, f.x), f.y);
}

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    // Середина точки маски в пикселях кадра, затем в пикселях картинки слоя. Точка маски
    // накрывает downscale × downscale пикселей кадра: четыре смеси (каждая
    // смешивает 2×2 пикселя) усредняют их все при уменьшении в 2 и 4 раза.
    float2 frame_pixel = input.position.xy * pc.downscale;
    float2 center = frame_pixel - float2(pc.offset);
    float q = pc.downscale * 0.25;
    float alpha = 0.0;
    alpha += layer_alpha(center + float2(-q, -q));
    alpha += layer_alpha(center + float2(q, -q));
    alpha += layer_alpha(center + float2(-q, q));
    alpha += layer_alpha(center + float2(q, q));
    return float4(alpha * 0.25, 0.0, 0.0, 1.0);
}

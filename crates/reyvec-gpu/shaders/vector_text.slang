// Текст векторного слоя. Буквы всех блоков текста лежат на видеокарте прямоугольниками
// (загружаются, только когда текст меняется), а их формы — в атласе MSDF: в каждом
// пикселе атласа не цвет, а расстояние до края буквы. Каждый кадр процессор пишет одну
// запись на блок (где стоит, какого цвета), а этот шейдер ставит все буквы блока одним
// вызовом: шесть вершин (два треугольника) на букву.
//
// Край буквы сглаживается здесь же: расстояние из атласа переводится в пиксели экрана,
// и пиксель закрашивается на долю, которую буква его покрывает. Поэтому буквы чёткие при
// любом размере и повороте, без MSAA.

struct Glyph
{
    float4 rect; // в координатах блока, y вверх: левый, верхний, правый, нижний
    float4 uv;   // в долях атласа: левый, верхний, правый, нижний
};

// Та же запись, что у фигур (`vector.slang`): блок текста рисуется как экземпляр.
struct Instance
{
    float4 linear;
    float2 offset;
    uint first_glyph; // первая буква блока в общем списке букв
    uint glyph_count;
    float4 color;     // цвет блока × оттенок экземпляра, линейный
    uint unused0;
    uint unused1;
    float unused2;
    uint unused3;
};

struct Push
{
    Glyph* glyphs;
    Instance* instances;
    float aspect;
    uint atlas;        // номер атласа в общем списке текстур
    float2 atlas_size; // в пикселях
    float range;       // сколько пикселей атласа охватывают расстояния (RANGE в reyvec-text)
};
[[vk::push_constant]] ConstantBuffer<Push> pc;

[[vk::binding(0, 0)]] Texture2D textures[];
[[vk::binding(1, 0)]] SamplerState samplers[2];

struct VertexOut
{
    float4 position : SV_Position;
    float2 uv : UV;
    float4 color : COLOR;
};

// Углы прямоугольника буквы: (0, 0) левый нижний, (1, 1) правый верхний.
static const float2 CORNERS[6] = {
    float2(0.0, 0.0), float2(1.0, 0.0), float2(1.0, 1.0),
    float2(0.0, 0.0), float2(1.0, 1.0), float2(0.0, 1.0)
};

[shader("vertex")]
VertexOut vs_main(uint vertex : SV_VulkanVertexID, uint instance : SV_VulkanInstanceID)
{
    Instance inst = pc.instances[instance];
    Glyph g = pc.glyphs[inst.first_glyph + vertex / 6];
    float2 c = CORNERS[vertex % 6];
    float2 local = float2(lerp(g.rect.x, g.rect.z, c.x), lerp(g.rect.w, g.rect.y, c.y));
    float2 p = float2(
        inst.linear.x * local.x + inst.linear.z * local.y,
        inst.linear.y * local.x + inst.linear.w * local.y) + inst.offset;
    // Сцена: y вверх от -1 до 1, x от -aspect до aspect.
    p.x /= pc.aspect;

    VertexOut o;
    o.position = float4(p.x, -p.y, 0.0, 1.0);
    o.uv = float2(lerp(g.uv.x, g.uv.z, c.x), lerp(g.uv.w, g.uv.y, c.y));
    o.color = inst.color;
    return o;
}

float median(float a, float b, float c)
{
    return max(min(a, b), min(max(a, b), c));
}

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    float4 s = textures[pc.atlas].Sample(samplers[0], input.uv);
    // Сколько пикселей экрана на один охват поля: чем крупнее буква на экране, тем
    // резче переход от расстояния к закраске.
    float2 unit = pc.range / pc.atlas_size;
    float2 screen = 1.0 / max(fwidth(input.uv), float2(1e-9, 1e-9));
    float px_range = max(0.5 * dot(unit, screen), 1.0);
    // Середина трёх каналов: так углы букв остаются острыми. Но вдали от буквы три
    // канала могут ошибочно сказать «внутри» (продолжения прямых краёв, видны тонкими
    // палочками у края прямоугольника буквы), поэтому сверху её ограничивает обычное
    // расстояние из четвёртого канала с запасом в пиксель атласа на острые углы.
    float sd = min(median(s.r, s.g, s.b), s.a + 1.0 / pc.range);
    float d = px_range * (sd - 0.5);
    float coverage = saturate(d + 0.5);
    return float4(input.color.rgb, input.color.a * coverage);
}

// Интерфейс игры (reyvec-ui, решение 0022): прямоугольники экрана и буквы одной командой.
//
// Процессор кладёт в буфер список прямоугольников в пикселях окна (`UiQuad` в reyvec-ui),
// шесть вершин на каждый. Скругления, обводку и край без ступенек считает этот шейдер по
// расстоянию до скруглённого прямоугольника, поэтому их не нужно резать на треугольники и
// они ровные в любом размере. Буквы — из атласа MSDF, как в `vector_text.slang`.
//
// Цвет выходит с умноженной прозрачностью (смешивание Premultiplied).

struct Quad
{
    float4 rect;   // пиксели окна: левый, верхний, правый, нижний
    float4 color;  // линейный, прозрачность не умножена
    float4 extra;  // блок: цвет обводки; буква: прямоугольник в долях атласа
    float4 params; // x: скругление, y: толщина обводки (пиксели), z: 0 — блок, 1 — буква
};

struct Push
{
    Quad* quads;
    float2 screen;     // размер окна в пикселях
    uint atlas;        // номер атласа букв в общем списке текстур
    float range;       // охват расстояний атласа в его пикселях (RANGE в reyvec-text)
    float2 atlas_size; // в пикселях
};
[[vk::push_constant]] ConstantBuffer<Push> pc;

[[vk::binding(0, 0)]] Texture2D textures[];
[[vk::binding(1, 0)]] SamplerState samplers[2];

struct VertexOut
{
    float4 position : SV_Position;
    float2 local : LOCAL;   // блок: пиксель от середины прямоугольника; буква: место в атласе
    nointerpolation float4 color : COLOR;
    nointerpolation float4 extra : EXTRA;
    nointerpolation float4 params : PARAMS; // x, y как у Quad; z — вид; w — не нужно
    nointerpolation float2 half_size : HALF;
};

static const float2 CORNERS[6] = {
    float2(0.0, 0.0), float2(1.0, 0.0), float2(1.0, 1.0),
    float2(0.0, 0.0), float2(1.0, 1.0), float2(0.0, 1.0)
};

[shader("vertex")]
VertexOut vs_main(uint vertex : SV_VulkanVertexID, uint instance : SV_VulkanInstanceID)
{
    Quad q = pc.quads[instance];
    float2 c = CORNERS[vertex % 6];
    bool glyph = q.params.z > 0.5;
    float2 lo = q.rect.xy;
    float2 hi = q.rect.zw;
    float2 center = 0.5 * (lo + hi);
    float2 half_size = 0.5 * (hi - lo);
    // Блок растёт на пиксель с каждой стороны: там его сглаженный край.
    float grow = glyph ? 0.0 : 1.0;
    float2 p = lerp(lo - grow, hi + grow, c);

    VertexOut o;
    o.position = float4(p / pc.screen * 2.0 - 1.0, 0.0, 1.0);
    o.local = glyph ? lerp(q.extra.xy, q.extra.zw, c) : p - center;
    o.color = q.color;
    o.extra = q.extra;
    o.params = q.params;
    o.half_size = half_size;
    return o;
}

// Расстояние до скруглённого прямоугольника с половинами `b` и скруглением `r`
// (меньше нуля — внутри).
float rounded_box(float2 p, float2 b, float r)
{
    r = min(r, min(b.x, b.y));
    float2 q = abs(p) - b + r;
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - r;
}

float median(float a, float b, float c)
{
    return max(min(a, b), min(max(a, b), c));
}

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    if (input.params.z > 0.5)
    {
        // Буква: как в vector_text.slang.
        float4 s = textures[pc.atlas].Sample(samplers[0], input.local);
        float2 unit = pc.range / pc.atlas_size;
        float2 screen_px = 1.0 / max(fwidth(input.local), float2(1e-9, 1e-9));
        float px_range = max(0.5 * dot(unit, screen_px), 1.0);
        float sd = min(median(s.r, s.g, s.b), s.a + 1.0 / pc.range);
        float coverage = saturate(px_range * (sd - 0.5) + 0.5);
        float a = input.color.a * coverage;
        return float4(input.color.rgb * a, a);
    }
    float d = rounded_box(input.local, input.half_size, input.params.x);
    float outer = saturate(0.5 - d);
    float border = input.params.y;
    float inner = border > 0.0 ? saturate(0.5 - (d + border)) : outer;
    float fill_a = input.color.a * inner;
    float border_a = input.extra.a * (outer - inner);
    return float4(input.color.rgb * fill_a + input.extra.rgb * border_a, fill_a + border_a);
}

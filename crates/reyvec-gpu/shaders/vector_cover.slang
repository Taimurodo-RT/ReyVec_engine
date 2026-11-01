// Способ 2, проход заливки: красит пиксели фигуры, где трафарет не ноль, и сразу
// обнуляет трафарет (это делает настройка конвейера, а не шейдер). Рисует те же
// треугольники веера и кривых, что и проход трафарета: вместе они накрывают всю фигуру.

struct CurveVertex
{
    float2 position;
    float2 uv;
};

#include "include/paint.slang"

struct Instance
{
    float4 linear;
    float2 offset;
    uint first_index;
    uint curve_first_index;
    float4 color;
    uint morph_stride; // перетекание: точек между ключевыми контурами в буфере
    uint morph_last;   // номер последнего ключевого контура (0 у обычных фигур)
    float morph;       // форма: 0 первый контур, 1 второй, 1.5 между вторым и третьим
    uint paint;        // градиент из списка закрасок (NO_PAINT у фигур одного цвета)
};

struct Push
{
    uint* indices;
    CurveVertex* vertices;
    Instance* instances;
    float aspect;
    uint samples;
    Paint* paints;
    GradientStop* stops;
};
[[vk::push_constant]] ConstantBuffer<Push> pc;

struct VertexOut
{
    float4 position : SV_Position;
    nointerpolation float4 color : COLOR;
    float2 local : LOCAL;
    nointerpolation uint paint : PAINT;
};

// Положение точки у перетекающей фигуры: смесь двух соседних ключевых контуров. Точки
// контура k лежат в буфере через k · morph_stride после точек первого.
float2 morphed(Instance inst, uint index, float2 first)
{
    if (inst.morph_last == 0)
        return first;
    float m = clamp(inst.morph, 0.0, float(inst.morph_last));
    uint k = min(uint(m), inst.morph_last - 1);
    float2 a = pc.vertices[index + k * inst.morph_stride].position;
    float2 b = pc.vertices[index + (k + 1) * inst.morph_stride].position;
    return lerp(a, b, m - float(k));
}

[shader("vertex")]
VertexOut vs_main(uint vertex : SV_VulkanVertexID, uint instance : SV_VulkanInstanceID)
{
    Instance inst = pc.instances[instance];
    uint index = pc.indices[inst.curve_first_index + vertex];
    CurveVertex v = pc.vertices[index];
    float2 shape = morphed(inst, index, v.position);
    float2 p = float2(
        inst.linear.x * shape.x + inst.linear.z * shape.y,
        inst.linear.y * shape.x + inst.linear.w * shape.y) + inst.offset;
    p.x /= pc.aspect;
    VertexOut o;
    o.position = float4(p.x, -p.y, 0.0, 1.0);
    o.color = inst.color;
    o.local = shape;
    o.paint = inst.paint;
    return o;
}

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    float4 color = input.color;
    if (input.paint != NO_PAINT)
        color *= paint_color(pc.paints, pc.stops, input.paint, input.local);
    return color;
}

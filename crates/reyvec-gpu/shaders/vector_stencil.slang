// Способ 2, проход трафарета: считает, сколько раз контур фигуры оборачивается вокруг
// каждой точки пикселя. Цвет не пишет (см. `curves.rs` в reyvec-vector).
//
// Треугольники веера всегда целиком внутри. У треугольника кривой точка внутри, если
// u² − v ≤ 0. Проверка идёт для каждой точки MSAA отдельно: так край кривой сглаживается
// так же, как прямые края, а не ступеньками по целым пикселям.

struct CurveVertex
{
    float2 position; // в координатах фигуры
    float2 uv;       // координаты кривой: (0,0), (½,0), (1,1); у веера (0,1)
};

struct Instance
{
    float4 linear;
    float2 offset;
    uint first_index;       // треугольники способа 1 (здесь не нужны)
    uint curve_first_index; // где начинаются треугольники веера и кривых фигуры
    float4 color;
    uint morph_stride; // перетекание: точек между ключевыми контурами в буфере
    uint morph_last;   // номер последнего ключевого контура (0 у обычных фигур)
    float morph;       // форма: 0 первый контур, 1 второй, 1.5 между вторым и третьим
    uint pad;
};

struct Push
{
    uint* indices;
    CurveVertex* vertices;
    Instance* instances;
    float aspect;
    uint samples; // точек MSAA на пиксель: 1, 2, 4 или 8
    uint unused0, unused1, unused2, unused3; // место градиентов в общих константах (здесь не нужны)
    float4 view_linear; // вид камеры (Camera2D в reyvec-vector): матрица 2×2 (a, b, c, d)
    float2 view_offset; // и сдвиг: сцена → экран, одинаково для всех фигур кадра
};
[[vk::push_constant]] ConstantBuffer<Push> pc;

struct VertexOut
{
    float4 position : SV_Position;
    float2 uv : CURVE;
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
    p = float2(
        pc.view_linear.x * p.x + pc.view_linear.z * p.y,
        pc.view_linear.y * p.x + pc.view_linear.w * p.y) + pc.view_offset;
    p.x /= pc.aspect;
    VertexOut o;
    o.position = float4(p.x, -p.y, 0.0, 1.0);
    o.uv = v.uv;
    return o;
}

// Стандартные положения точек MSAA в Vulkan (доли пикселя от левого верхнего угла).
static const float2 SAMPLES_2[2] = { float2(0.75, 0.75), float2(0.25, 0.25) };
static const float2 SAMPLES_4[4] = {
    float2(0.375, 0.125), float2(0.875, 0.375), float2(0.125, 0.625), float2(0.625, 0.875)
};
static const float2 SAMPLES_8[8] = {
    float2(0.5625, 0.3125), float2(0.4375, 0.6875), float2(0.8125, 0.5625), float2(0.3125, 0.1875),
    float2(0.1875, 0.8125), float2(0.0625, 0.4375), float2(0.6875, 0.9375), float2(0.9375, 0.0625)
};

bool inside(float2 uv)
{
    return uv.x * uv.x - uv.y <= 0.0;
}

[shader("fragment")]
float4 fs_main(VertexOut input, out uint coverage : SV_Coverage) : SV_Target
{
    // uv меняется по треугольнику линейно, поэтому в любой точке пикселя его можно
    // получить из значения в центре и изменения к соседним пикселям.
    float2 dx = ddx(input.uv);
    float2 dy = ddy(input.uv);
    uint mask = 0;
    if (pc.samples >= 8)
    {
        for (uint i = 0; i < 8; i++)
        {
            float2 o = SAMPLES_8[i] - 0.5;
            if (inside(input.uv + dx * o.x + dy * o.y)) mask |= 1u << i;
        }
    }
    else if (pc.samples >= 4)
    {
        for (uint i = 0; i < 4; i++)
        {
            float2 o = SAMPLES_4[i] - 0.5;
            if (inside(input.uv + dx * o.x + dy * o.y)) mask |= 1u << i;
        }
    }
    else if (pc.samples >= 2)
    {
        for (uint i = 0; i < 2; i++)
        {
            float2 o = SAMPLES_2[i] - 0.5;
            if (inside(input.uv + dx * o.x + dy * o.y)) mask |= 1u << i;
        }
    }
    else if (inside(input.uv))
    {
        mask = 1;
    }
    // Маска 0 ничего не пишет: это и есть «отбросить пиксель» (без discard, которому
    // нужна отдельная возможность видеокарты).
    coverage = mask;
    return float4(0.0, 0.0, 0.0, 0.0);
}

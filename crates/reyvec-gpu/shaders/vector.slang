// Векторные фигуры, разбитые на треугольники. Точки и номера точек всех фигур лежат в
// общих буферах на видеокарте (загружаются один раз). Каждый кадр процессор пишет только
// список экземпляров: какая фигура, где стоит, какого цвета. Двигает и поворачивает
// фигуры этот шейдер.
//
// Края обводок шейдер сглаживает сам: треугольники обводки чуть шире настоящей линии, а
// по данным края (см. `EdgeData` в reyvec-vector) шейдер знает расстояние от пикселя до
// края линии и закрашивает пиксель на долю, которую линия его покрывает. Края заливок
// сглаживает MSAA.

struct Vertex
{
    float2 position; // в координатах фигуры
    float2 offset;   // смещение от точки контура (для круглых стыков и концов)
    float4 edge;     // поперёк линии, полутолщина (0 у заливок), вид края, 0
};

struct Instance
{
    float4 linear;    // матрица 2×2 (a, b, c, d): x' = a·x + c·y, y' = b·x + d·y
    float2 offset;    // сдвиг (e, f)
    uint first_index; // где в общем списке номеров начинаются треугольники фигуры
    uint curve_first_index; // треугольники способа 2 (трафарет), см. vector_stencil.slang
    float4 color;     // линейный цвет с альфой (цвет фигуры × оттенок экземпляра)
};

struct Push
{
    uint* indices;      // номера точек всех фигур, по три на треугольник
    Vertex* vertices;   // точки всех фигур в их собственных координатах
    Instance* instances;
    float aspect;
};
[[vk::push_constant]] ConstantBuffer<Push> pc;

struct VertexOut
{
    float4 position : SV_Position;
    float4 color : COLOR;
    float2 offset : OFFSET;
    float4 edge : EDGE;
};

[shader("vertex")]
VertexOut vs_main(uint vertex : SV_VulkanVertexID, uint instance : SV_VulkanInstanceID)
{
    // Одна команда рисования = подряд идущие экземпляры одной фигуры. Номер экземпляра
    // уже включает начало этого отрезка (firstInstance команды).
    Instance inst = pc.instances[instance];
    Vertex v = pc.vertices[pc.indices[inst.first_index + vertex]];
    float2 local = v.position;
    float2 p = float2(
        inst.linear.x * local.x + inst.linear.z * local.y,
        inst.linear.y * local.x + inst.linear.w * local.y) + inst.offset;
    // Сцена: y вверх от -1 до 1, x от -aspect до aspect (как у спрайтов).
    p.x /= pc.aspect;

    VertexOut o;
    o.position = float4(p.x, -p.y, 0.0, 1.0);
    o.color = inst.color;
    o.offset = v.offset;
    o.edge = v.edge;
    return o;
}

// Доля пикселя по одну сторону края, если `f` — расстояние до края (больше нуля внутри).
// Деление на изменение `f` между соседними пикселями переводит расстояние в пиксели при
// любом масштабе и повороте фигуры.
float side_coverage(float f)
{
    float per_pixel = length(float2(ddx(f), ddy(f)));
    return saturate(f / max(per_pixel, 1e-12) + 0.5);
}

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    float across = abs(input.edge.x);
    float half_width = input.edge.y;
    float kind = input.edge.z;
    float radial = length(input.offset);
    // Расстояние от середины линии: у прямых краёв поперёк, у круглых концов до точки
    // контура, у круглых стыков меньшее из двух.
    float r = kind > 1.5 ? radial : (kind > 0.5 ? min(across, radial) : across);
    // Оба края линии сразу: так линия тоньше пикселя бледнеет, а не пропадает.
    float coverage = saturate(side_coverage(half_width - r) + side_coverage(half_width + r) - 1.0);
    if (half_width <= 0.0)
    {
        coverage = 1.0;
    }
    return float4(input.color.rgb, input.color.a * coverage);
}

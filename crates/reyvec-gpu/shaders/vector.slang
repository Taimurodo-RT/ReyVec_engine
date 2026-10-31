// Векторные фигуры, разбитые на треугольники. Точки и номера точек всех фигур лежат в
// общих буферах на видеокарте (загружаются один раз). Каждый кадр процессор пишет только
// список экземпляров: какая фигура, где стоит, какого цвета. Двигает и поворачивает
// фигуры этот шейдер.

struct Instance
{
    float4 linear;    // матрица 2×2 (a, b, c, d): x' = a·x + c·y, y' = b·x + d·y
    float2 offset;    // сдвиг (e, f)
    uint first_index; // где в общем списке номеров начинаются треугольники фигуры
    uint pad;
    float4 color;     // линейный цвет с альфой (цвет фигуры × оттенок экземпляра)
};

struct Push
{
    uint* indices;      // номера точек всех фигур, по три на треугольник
    float2* vertices;   // точки всех фигур в их собственных координатах
    Instance* instances;
    float aspect;
};
[[vk::push_constant]] ConstantBuffer<Push> pc;

struct VertexOut
{
    float4 position : SV_Position;
    float4 color : COLOR;
};

[shader("vertex")]
VertexOut vs_main(uint vertex : SV_VulkanVertexID, uint instance : SV_VulkanInstanceID)
{
    // Одна команда рисования = подряд идущие экземпляры одной фигуры. Номер экземпляра
    // уже включает начало этого отрезка (firstInstance команды).
    Instance inst = pc.instances[instance];
    float2 local = pc.vertices[pc.indices[inst.first_index + vertex]];
    float2 p = float2(
        inst.linear.x * local.x + inst.linear.z * local.y,
        inst.linear.y * local.x + inst.linear.w * local.y) + inst.offset;
    // Сцена: y вверх от -1 до 1, x от -aspect до aspect (как у спрайтов).
    p.x /= pc.aspect;

    VertexOut o;
    o.position = float4(p.x, -p.y, 0.0, 1.0);
    o.color = inst.color;
    return o;
}

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    return input.color;
}

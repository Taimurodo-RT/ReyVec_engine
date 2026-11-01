// Маска теней (задача 4.2): какая доля каждой точки закрыта фигурой, отбрасывающей тень
// (1 внутри, 0 снаружи, плавно на краю шириной в точку). Рисуется в уменьшенном
// разрешении; из неё jump flooding (jfa_*.slang) строит поле расстояний. Доля нужна,
// чтобы поле знало, где край внутри точки: без неё край привязан к сетке маски и линии
// поля идут ступеньками (узкое место света 4).
// Здесь SDF-фигуры сцены «свет»; силуэты векторных слоёв с отметкой «отбрасывает тень»
// дорисовывает поверх light_mask_layer.slang (4.2б).

#include "include/light_demo.slang"

struct Push
{
    float time;
    float aspect;
    float2 center;
    float zoom;
    float height; // высота полного кадра в пикселях (не маски)
};
[[vk::push_constant]] ConstantBuffer<Push> pc;

struct VertexOut
{
    float4 position : SV_Position;
    float2 screen : SCREEN;
};

[shader("vertex")]
VertexOut vs_main(uint vertex : SV_VulkanVertexID)
{
    float2 uv = float2((vertex << 1) & 2, vertex & 2);
    float2 ndc = uv * 2.0 - 1.0;
    VertexOut o;
    o.position = float4(ndc, 0.0, 1.0);
    o.screen = float2(ndc.x * pc.aspect, -ndc.y);
    return o;
}

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    float2 p = pc.center + input.screen / pc.zoom;
    // Размер точки маски в единицах сцены: на сколько сдвигается p к соседней точке.
    float point = abs(ddx(p.x));
    float covered = sdf_coverage(demo_field(p, pc.time), point);
    return float4(covered, 0.0, 0.0, 1.0);
}

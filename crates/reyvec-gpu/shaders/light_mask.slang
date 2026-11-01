// Маска теней (задача 4.2): 1 там, где фигура, отбрасывающая тень, 0 снаружи. Рисуется
// в половинном разрешении; из неё jump flooding (jfa_*.slang) строит поле расстояний.
// Пока фигуры — те же SDF-фигуры сцены «свет»; векторные фигуры с отметкой «отбрасывает
// тень» добавятся в 4.2б.

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
    float inside = demo_field(p, pc.time) < 0.0 ? 1.0 : 0.0;
    return float4(inside, 0.0, 0.0, 1.0);
}

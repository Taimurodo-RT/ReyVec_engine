// Карты теней ламп (7.9б, 2.5D-вид уровня треугольниками): те же треугольники, что в
// проходе «полигоны 3D» (mesh3d.slang), но глазами лампы. Лампы стоят перед уровнем и
// светят на него, поэтому каждой хватает одной карты с широким углом (не куба из шести).
// Карты лежат рядом в одной картинке: лампа k — квадрат k по горизонтали, рисуется с
// экземпляром k. В точке — расстояние от лампы до ближней поверхности (R32F); «тени 3D»
// (include/shadow3d.slang, map_shadow) сравнивают с ним расстояние до своей точки.

#define SPACE_OWN_VERTEX
#include "include/space.slang"

static const uint VERTEX_WORDS = 4;

struct ShadowOut
{
    float4 position : SV_Position;
    float3 world : WORLD;
    nointerpolation uint lamp : LAMP;
};

[shader("vertex")]
ShadowOut vs_main(uint vertex : SV_VulkanVertexID, uint lamp : SV_VulkanInstanceID)
{
    uint at = pc.scene[7];
    uint count = pc.scene[at];
    ShadowOut o;
    o.lamp = lamp;
    if (vertex >= count || lamp >= min(pc.scene[at + 1], SHADOW_LAMPS))
    {
        o.position = float4(0.0, 0.0, -1.0, 1.0);
        o.world = float3(0.0, 0.0, 0.0);
        return o;
    }
    uint v = at + MESH_HEADER_WORDS + vertex * VERTEX_WORDS;
    float3 p = float3(asfloat(pc.scene[v]), asfloat(pc.scene[v + 1]), asfloat(pc.scene[v + 2]));
    uint m = at + MESH_SHADOW_PROJ + lamp * 16;
    float4 clip = float4(0.0, 0.0, 0.0, 0.0);
    for (uint c = 0; c < 4; c++)
    {
        float4 column = float4(asfloat(pc.scene[m + 4 * c]), asfloat(pc.scene[m + 4 * c + 1]),
                               asfloat(pc.scene[m + 4 * c + 2]), asfloat(pc.scene[m + 4 * c + 3]));
        clip += column * (c < 3 ? p[c] : 1.0);
    }
    o.position = float4(clip.x, -clip.y, clip.z, clip.w);
    o.world = p;
    return o;
}

[shader("fragment")]
float4 fs_main(ShadowOut input) : SV_Target
{
    Lamp l = read_lamp(input.lamp);
    return float4(length(input.world - l.at), 0.0, 0.0, 0.0);
}

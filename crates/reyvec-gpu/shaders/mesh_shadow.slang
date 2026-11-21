// Карты теней ламп (7.9б, 2.5D-вид уровня треугольниками): те же треугольники, что в
// проходе «полигоны 3D» (mesh3d.slang), но глазами лампы. Лампы стоят перед уровнем и
// светят на него, поэтому каждой хватает одной карты с широким углом (не куба из шести).
// Карты лежат рядом в одной картинке: лампа k — квадрат k по горизонтали, рисуется с
// экземпляром k. В точке — расстояние от лампы до ближней поверхности (R32F); «тени 3D»
// (include/shadow3d.slang, map_shadow) сравнивают с ним расстояние до своей точки.
// Солнце (8.20в, режим «треугольники» 3D) — квадраты сразу после ламп (ближний кусок
// взгляда и дальний), прямоугольная проекция вдоль солнца; в точке — глубина от солнца,
// −(точка · SUN).

#define SPACE_OWN_VERTEX
#include "include/space.slang"


struct ShadowOut
{
    float4 position : SV_Position;
    float3 world : WORLD;
    nointerpolation uint lamp : LAMP;
};

ShadowOut hidden(uint lamp)
{
    ShadowOut o;
    o.position = float4(0.0, 0.0, -1.0, 1.0);
    o.world = float3(0.0, 0.0, 0.0);
    o.lamp = lamp;
    return o;
}

// Экземпляр: старшие биты (с 20-го) — квадрат карты (лампа; квадрат после ламп — солнце,
// 8.20в), младшие — место сетки из формы (0 — модели, вершины из раздела моделей).
[shader("vertex")]
ShadowOut vs_main(uint vertex : SV_VulkanVertexID, uint instance : SV_VulkanInstanceID)
{
    uint at = pc.scene[7];
    uint lamp = instance >> 20;
    uint shape = instance & 0xfffffu;
    uint lamps = min(pc.scene[at + 1], SHADOW_LAMPS);
    bool sun = lamp >= lamps;
    // Карты солнца — квадраты сразу после ламп, по одной на кусок взгляда (ближняя первой).
    uint cascade = lamp - lamps;
    if (sun && cascade >= min(pc.scene[at + MESH_SUN], SUN_CASCADES))
        return hidden(lamp);
    float3 p;
    if (shape > 0)
    {
        float3 normal;
        uint material;
        if (!shape_point(at, vertex, shape, p, normal, material))
            return hidden(lamp);
    }
    else
    {
        if (vertex >= pc.scene[at])
            return hidden(lamp);
        uint v = at + MESH_HEADER_WORDS + vertex * MESH_VERTEX_WORDS;
        p = float3(asfloat(pc.scene[v]), asfloat(pc.scene[v + 1]), asfloat(pc.scene[v + 2]));
    }
    uint m = sun ? at + MESH_SUN_PROJ + cascade * 16 : at + MESH_SHADOW_PROJ + lamp * 16;
    float4 clip = float4(0.0, 0.0, 0.0, 0.0);
    for (uint c = 0; c < 4; c++)
    {
        float4 column = float4(asfloat(pc.scene[m + 4 * c]), asfloat(pc.scene[m + 4 * c + 1]),
                               asfloat(pc.scene[m + 4 * c + 2]), asfloat(pc.scene[m + 4 * c + 3]));
        clip += column * (c < 3 ? p[c] : 1.0);
    }
    ShadowOut o;
    o.position = float4(clip.x, -clip.y, clip.z, clip.w);
    o.world = p;
    o.lamp = lamp;
    return o;
}

// Лампа — расстояние от неё; солнце — глубина от солнца −(точка · SUN): у обоих меньше —
// ближе к свету, пустое место карты — 1e9.
[shader("fragment")]
float4 fs_main(ShadowOut input) : SV_Target
{
    uint at = pc.scene[7];
    if (input.lamp >= min(pc.scene[at + 1], SHADOW_LAMPS))
        return float4(-dot(input.world, SUN), 0.0, 0.0, 0.0);
    Lamp l = read_lamp(input.lamp);
    float d = length(input.world - l.at);
    // Сцена «3D» (8.20в2): шарик-лампочка вокруг лампы (форма с сеткой) не заслоняет свою
    // же лампу — ближе 35 см к ней ничего не рисуется, как луч тени не доходит до лампы
    // 35 см (shadow_part). Иначе вся карта — шарик, и в ней всё в тени.
    if ((pc.scene[11] & SCENE_LAMP_MAPS_DOWN) != 0 && d < LAMP_SHADOW_GAP)
        discard;
    return float4(d, 0.0, 0.0, 0.0);
}

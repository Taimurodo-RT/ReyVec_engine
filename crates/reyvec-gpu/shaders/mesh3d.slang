// Сцена «3D», проход «полигоны 3D» (опыт совмещения 7.6ж): полигональные модели
// растеризацией с глубиной в свой буфер геометрии — той же раскладки, что у
// «реймарчинга» (расстояние вдоль луча, материал, нормаль). «Реймарчинг» не шагает
// дальше этих точек и берёт их, если луч раньше ни во что не попал; тени, затенение
// в углах и свет дальше общие (raymarch_shadow.slang, raymarch_shade.slang).
//
// Вершины лежат в буфере сцены (crates/reyvec-gpu/src/space.rs): слово 7 заголовка —
// начало раздела моделей; в разделе число вершин, 3 запасных, матрица «мир → экран»
// столбцами, потом вершины по 4 слова (место и номер материала), по 3 на треугольник.
// Грани плоские: нормаль — по наклону соседних точек.

#define SPACE_OWN_VERTEX
#include "include/space.slang"

static const uint MESH_HEADER_WORDS = 20;
static const uint VERTEX_WORDS = 4;

struct MeshOut
{
    float4 position : SV_Position;
    float3 world : WORLD;
    nointerpolation uint material : MATERIAL;
};

[shader("vertex")]
MeshOut vs_main(uint vertex : SV_VulkanVertexID)
{
    uint at = pc.scene[7];
    uint count = pc.scene[at];
    MeshOut o;
    if (vertex >= count)
    {
        // Испорченный буфер: точка за кадром, треугольник не рисуется.
        o.position = float4(0.0, 0.0, -1.0, 1.0);
        o.world = float3(0.0, 0.0, 0.0);
        o.material = 0;
        return o;
    }
    uint v = at + MESH_HEADER_WORDS + vertex * VERTEX_WORDS;
    float3 p = float3(asfloat(pc.scene[v]), asfloat(pc.scene[v + 1]), asfloat(pc.scene[v + 2]));
    uint m = at + 4;
    float4 clip = float4(0.0, 0.0, 0.0, 0.0);
    for (uint c = 0; c < 4; c++)
    {
        float4 column = float4(asfloat(pc.scene[m + 4 * c]), asfloat(pc.scene[m + 4 * c + 1]),
                               asfloat(pc.scene[m + 4 * c + 2]), asfloat(pc.scene[m + 4 * c + 3]));
        clip += column * (c < 3 ? p[c] : 1.0);
    }
    // У камеры y экрана вверх, у Vulkan — вниз.
    o.position = float4(clip.x, -clip.y, clip.z, clip.w);
    o.world = p;
    o.material = pc.scene[v + 3];
    return o;
}

[shader("fragment")]
float4 fs_main(MeshOut input) : SV_Target
{
    // Луч этого пикселя — как у «реймарчинга»: расстояние считается от его начала.
    float2 size = float2(round(pc.aspect * pc.height), pc.height);
    float2 s = float2(input.position.x / size.x * 2.0 - 1.0, 1.0 - input.position.y / size.y * 2.0);
    Ray ray = screen_ray(s);
    float t = dot(input.world - ray.origin, ray.dir);
    float3 n = normalize(cross(ddx(input.world), ddy(input.world)));
    if (dot(n, ray.dir) > 0.0)
        n = -n;
    return float4(t, float(input.material), oct_encode(n));
}

// Сцена «3D», проход «полигоны 3D» (опыт совмещения 7.6ж): полигональные модели
// растеризацией с глубиной в свой буфер геометрии — то же, что у «реймарчинга»
// (расстояние вдоль луча, материал, нормаль), упакованное в 8 байт (pack_mesh).
// «Реймарчинг» не шагает дальше этих точек и берёт их, если луч раньше ни во что не
// попал; тени, затенение в углах и свет дальше общие (raymarch_shadow.slang,
// raymarch_shade.slang).
//
// Вершины лежат в буфере сцены (crates/reyvec-gpu/src/space.rs): слово 7 заголовка —
// начало раздела моделей (раскладка — MESH_HEADER_WORDS в space.slang), потом вершины
// по 5 слов (место, слово материала и точка текстуры), по 3 на треугольник.
// Слово материала без бита 31 — просто номер: грань плоская, нормаль — по наклону соседних
// точек. С битом 31 (модели glTF с нормалями, 8.14б) — раскладка pack_mesh: 11 бит
// материала и нормаль вершины; нормаль плавно меняется по грани (гладкие грани).
//
// Текстуры цвета (8.14б2): вариант mesh3d_color.slang (MESH_COLOR) пишет ещё картинку
// «цвет полигонов 3D» — цвет текстуры точки (у материала с флагом MATERIAL_TEXTURED,
// номер картинки — слово 5 записи материала), у остальных белый. Освещение умножает на
// него цвет материала (shade3d.slang, mesh_tint). Без текстур в кадре движок берёт этот
// файл как есть: лишней картинки нет.

#define SPACE_OWN_VERTEX
#include "include/space.slang"

struct MeshOut
{
    float4 position : SV_Position;
    float3 world : WORLD;
    nointerpolation uint material : MATERIAL;
    // Нормаль вершины; нулевая — грань плоская.
    float3 normal : NORMAL;
    // Точка текстуры (8.14б2).
    float2 uv : TEXCOORD;
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
        o.normal = float3(0.0, 0.0, 0.0);
        o.uv = float2(0.0, 0.0);
        return o;
    }
    uint v = at + MESH_HEADER_WORDS + vertex * MESH_VERTEX_WORDS;
    float3 p = float3(asfloat(pc.scene[v]), asfloat(pc.scene[v + 1]), asfloat(pc.scene[v + 2]));
    uint m = at + MESH_VIEW_PROJ;
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
    uint word = pc.scene[v + 3];
    if ((word & 0x80000000u) != 0)
    {
        float4 m = unpack_mesh(uint2(0u, word));
        o.material = uint(m.y);
        o.normal = oct_decode(m.zw);
    }
    else
    {
        o.material = word;
        o.normal = float3(0.0, 0.0, 0.0);
    }
    uint uv = pc.scene[v + 4];
    o.uv = float2(f16tof32(uv & 0xffff), f16tof32(uv >> 16));
    return o;
}

// Точка — упакованной (pack_mesh, 7.18е2). Без затенения по образцам: при MSAA все
// накрытые образцы пикселя получают одно и то же — расстояние луча середины пикселя до
// плоскости грани; лучи краёв пересчитывают его для своей точки (march.slang).
uint4 mesh_point(MeshOut input)
{
    // Луч этого пикселя — как у «реймарчинга»: расстояние считается от его начала.
    float2 size = float2(round(pc.aspect * pc.height), pc.height);
    float2 s = float2(input.position.x / size.x * 2.0 - 1.0, 1.0 - input.position.y / size.y * 2.0);
    Ray ray = screen_ray(s);
    float t = dot(input.world - ray.origin, ray.dir);
    float3 n = dot(input.normal, input.normal) > 1e-8
        ? normalize(input.normal)
        : normalize(cross(ddx(input.world), ddy(input.world)));
    if (dot(n, ray.dir) > 0.0)
        n = -n;
    return uint4(pack_mesh(t, input.material, n), 0, 0);
}

#ifdef MESH_COLOR
struct MeshTargets
{
    uint4 mesh : SV_Target0;
    float4 color : SV_Target1;
};

// Цвет текстуры точки (белый у материала без текстуры). Сглаживающий сэмплер с повтором
// (тот же, что у слоёв поля, binding 4) и уровни уменьшения текстуры: вдали не рябит.
[shader("fragment")]
MeshTargets fs_main(MeshOut input)
{
    MeshTargets o;
    o.mesh = mesh_point(input);
    uint material = min(input.material, max(pc.scene[3], 1) - 1);
    uint m = pc.scene[2] + material * 8;
    // Наклон точки текстуры к соседним пикселям — до ветвления: внутри него соседние
    // пиксели могут не считаться, и уровень текстуры вышел бы любым.
    float2 dx = ddx(input.uv), dy = ddy(input.uv);
    o.color = float4(1.0, 1.0, 1.0, 1.0);
    if ((pc.scene[m + 3] & MATERIAL_TEXTURED) != 0)
    {
        // У соседних треугольников бывают разные текстуры: номер не общий для группы.
        uint slot = NonUniformResourceIndex(pc.scene[m + 5]);
        o.color = float4(textures[slot].SampleGrad(field_sampler, input.uv, dx, dy).rgb, 1.0);
    }
    return o;
}
#else
[shader("fragment")]
uint4 fs_main(MeshOut input) : SV_Target
{
    return mesh_point(input);
}
#endif

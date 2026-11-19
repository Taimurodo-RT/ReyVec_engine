// Сцена «3D», проход «полигоны 3D» (опыт совмещения 7.6ж): полигональные модели
// растеризацией с глубиной в свой буфер геометрии — то же, что у «реймарчинга»
// (расстояние вдоль луча, материал, нормаль), упакованное в 8 байт (pack_mesh).
// «Реймарчинг» не шагает дальше этих точек и берёт их, если луч раньше ни во что не
// попал; тени, затенение в углах и свет дальше общие (raymarch_shadow.slang,
// raymarch_shade.slang).
//
// Сетки из форм (8.20б, режим «треугольники» для 3D) рисуются тем же проходом, своими
// вызовами с firstInstance ≥ 1 (shape_vertex).
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

// Число половинной точности из 16 бит (как f16_bits в space.rs). Не f16tof32: тот
// требует от видеокарты чисел half (shaderFloat16, shaderInt16), а их умеют не все.
float half_bits(uint h)
{
    float sign = (h & 0x8000u) != 0 ? -1.0 : 1.0;
    uint e = (h >> 10) & 0x1fu;
    uint m = h & 0x3ffu;
    if (e == 0)
        return sign * float(m) * (1.0 / 16777216.0); // денормальное: m · 2^−24
    if (e == 31)
        return sign * 65504.0; // бесконечность и не-число — самое большое число
    return sign * asfloat(((e + 112u) << 23) | (m << 13));
}

// Матрица камеры из раздела моделей: точка мира → точка отсечения.
float4 to_clip(uint at, float3 p)
{
    uint m = at + MESH_VIEW_PROJ;
    float4 clip = float4(0.0, 0.0, 0.0, 0.0);
    for (uint c = 0; c < 4; c++)
    {
        float4 column = float4(asfloat(pc.scene[m + 4 * c]), asfloat(pc.scene[m + 4 * c + 1]),
                               asfloat(pc.scene[m + 4 * c + 2]), asfloat(pc.scene[m + 4 * c + 3]));
        clip += column * (c < 3 ? p[c] : 1.0);
    }
    // У камеры y экрана вверх, у Vulkan — вниз.
    return float4(clip.x, -clip.y, clip.z, clip.w);
}

MeshOut nothing()
{
    // Испорченный буфер: точка за кадром, треугольник не рисуется.
    MeshOut o;
    o.position = float4(0.0, 0.0, -1.0, 1.0);
    o.world = float3(0.0, 0.0, 0.0);
    o.material = 0;
    o.normal = float3(0.0, 0.0, 0.0);
    o.uv = float2(0.0, 0.0);
    return o;
}

// Сетка из формы (8.20б): номер `vertex` — место в списке номеров вершин буфера сеток
// (первый номер сетки приходит как firstVertex), место сетки — `instance` − 1 (0 —
// модели). Буфер: слово 0 — начало вершин, номера, вершины по 4 слова (место, слово
// материала с нормалью, раскладка pack_mesh).
MeshOut shape_vertex(uint at, uint vertex, uint instance)
{
    uint2 a = uint2(pc.scene[at + MESH_SHAPES], pc.scene[at + MESH_SHAPES + 1]);
    uint count = pc.scene[at + MESH_SHAPES + 3];
    if (all(a == 0) || instance > count)
        return nothing();
    uint* shapes = bit_cast<uint*>(a);
    uint v = shapes[0] + shapes[1 + vertex] * 4;
    float3 local = float3(asfloat(shapes[v]), asfloat(shapes[v + 1]), asfloat(shapes[v + 2]));
    uint i = pc.scene[at + MESH_SHAPES + 2] + (instance - 1) * SHAPE_INSTANCE_WORDS;
    float3 c0 = float3(asfloat(pc.scene[i]), asfloat(pc.scene[i + 1]), asfloat(pc.scene[i + 2]));
    float3 c1 = float3(asfloat(pc.scene[i + 3]), asfloat(pc.scene[i + 4]), asfloat(pc.scene[i + 5]));
    float3 c2 = float3(asfloat(pc.scene[i + 6]), asfloat(pc.scene[i + 7]), asfloat(pc.scene[i + 8]));
    float3 shift = float3(asfloat(pc.scene[i + 9]), asfloat(pc.scene[i + 10]), asfloat(pc.scene[i + 11]));
    float3 p = c0 * local.x + c1 * local.y + c2 * local.z + shift;
    float4 m = unpack_mesh(uint2(0u, shapes[v + 3]));
    float3 n = oct_decode(m.zw);
    MeshOut o;
    o.position = to_clip(at, p);
    o.world = p;
    o.material = uint(m.y);
    // Увеличение у форм одинаково по осям: нормаль поворачивается той же матрицей.
    o.normal = c0 * n.x + c1 * n.y + c2 * n.z;
    o.uv = float2(0.0, 0.0);
    return o;
}

[shader("vertex")]
MeshOut vs_main(uint vertex : SV_VulkanVertexID, uint instance : SV_VulkanInstanceID)
{
    uint at = pc.scene[7];
    if (instance > 0)
        return shape_vertex(at, vertex, instance);
    uint count = pc.scene[at];
    if (vertex >= count)
        return nothing();
    uint v = at + MESH_HEADER_WORDS + vertex * MESH_VERTEX_WORDS;
    float3 p = float3(asfloat(pc.scene[v]), asfloat(pc.scene[v + 1]), asfloat(pc.scene[v + 2]));
    MeshOut o;
    o.position = to_clip(at, p);
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
    o.uv = float2(half_bits(uv & 0xffff), half_bits(uv >> 16));
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

// Объекты целиком из SDF (задача 4.5): слизь, капли, магия. Объект — несколько частей
// (кругов и капсул), слитых плавно; список частей каждый кадр пишет процессор
// (src/blobs.rs, там же зеркало формы на процессоре). Каждый объект рисуется одним
// прямоугольником по своим границам, поэтому форму считают только пиксели рядом с ним
// (а не весь экран, как в сцене «свет», узкое место света 1).
// mode 0: цвет в кадр сцены. Объект выглядит объёмным: наклон поверхности берётся из
//   наклона поля расстояний (у края поверхность круто уходит вниз, к середине выходит
//   на плоскость), отсюда мягкая тень сбоку и блик. Свет ламп потом умножает вывод.
// mode 1: доля точки, закрытая объектом, в маску теней (как силуэты векторных слоёв).

#include "include/sdf2d.slang"

struct Blob
{
    float2 a;
    float2 b;
    float r;
    float pad0;
    float pad1;
    float pad2;
};

struct Object
{
    uint first;   // первая часть в списке частей
    uint count;
    float k;      // плавность слияния частей (размер перемычки)
    uint flags;   // OBJECT_EMISSIVE, OBJECT_CASTS_SHADOW
    float4 color; // линейный
    float2 lo;    // границы объекта в координатах сцены
    float2 hi;
};

static const uint OBJECT_EMISSIVE = 1;
static const uint OBJECT_CASTS_SHADOW = 2;

struct Push
{
    Object* objects;
    Blob* blobs;
    float2 center; // камера: точка сцены в центре экрана
    float aspect;
    float zoom;
    uint mode;
    float pad;
};
[[vk::push_constant]] ConstantBuffer<Push> pc;

struct VertexOut
{
    float4 position : SV_Position;
    float2 p : SCENE;                  // точка сцены
    nointerpolation uint object : OBJECT;
};

[shader("vertex")]
VertexOut vs_main(uint vertex : SV_VulkanVertexID, uint instance : SV_VulkanInstanceID)
{
    Object o = pc.objects[instance];
    // Два треугольника прямоугольника границ.
    uint corners[6] = { 0, 1, 2, 2, 1, 3 };
    uint c = corners[vertex];
    float2 p = float2((c & 1) != 0 ? o.hi.x : o.lo.x, (c & 2) != 0 ? o.hi.y : o.lo.y);
    float2 s = (p - pc.center) * pc.zoom;
    VertexOut result;
    result.position = float4(s.x / pc.aspect, -s.y, 0.0, 1.0);
    // В маску теней попадают только объекты, отбрасывающие тень: остальные сжимаются в
    // точку и не рисуются.
    if (pc.mode == 1 && (o.flags & OBJECT_CASTS_SHADOW) == 0)
        result.position = float4(2.0, 2.0, 0.0, 1.0);
    result.p = p;
    result.object = instance;
    return result;
}

// Расстояние со знаком до объекта (внутри отрицательное). Зеркало: SdfObject::distance.
float object_field(Object o, float2 p)
{
    float d = 1e9;
    for (uint i = 0; i < o.count; i++)
    {
        Blob b = pc.blobs[o.first + i];
        d = op_smooth_union(d, sd_segment(p, b.a, b.b) - b.r, o.k);
    }
    return d;
}

// Откуда падает «свет формы» (для объёма; настоящие лампы умножают вывод отдельно):
// сверху слева, немного на зрителя.
static const float3 SHAPE_LIGHT = float3(-0.45, 0.6, 0.66);

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    Object o = pc.objects[input.object];
    float2 p = input.p;
    float pixel = max(abs(ddx(p.x)), 1e-6);
    float d = object_field(o, p);
    float cover = sdf_coverage(d, pixel);
    if (pc.mode == 1)
        return float4(cover, 0.0, 0.0, 1.0);
    // Снаружи объекта (большая часть прямоугольника) больше ничего не считаем.
    if (cover <= 0.0)
        return float4(0.0, 0.0, 0.0, 0.0);

    // Толщина: у края 0, глубже BEVEL — 1. Поверхность как у капли: у края почти
    // вертикальная, к середине горизонтальная.
    const float BEVEL = 0.07;
    float depth = saturate(-d / BEVEL);
    float flat_part = 1.0 - (1.0 - depth) * (1.0 - depth);
    float3 normal = float3(0.0, 0.0, 1.0);
    if (depth < 1.0)
    {
        // Наклон поля: куда расстояние растёт быстрее всего (наружу от поверхности).
        // Нужен только у края, где поверхность наклонена; в середине она плоская.
        float e = pixel;
        float2 grad = float2(
            object_field(o, p + float2(e, 0.0)) - object_field(o, p - float2(e, 0.0)),
            object_field(o, p + float2(0.0, e)) - object_field(o, p - float2(0.0, e)));
        float2 outward = grad / max(length(grad), 1e-6);
        normal = normalize(float3(outward * (1.0 - flat_part), flat_part + 0.05));
    }
    float3 to_light = normalize(SHAPE_LIGHT);

    float3 color;
    if ((o.flags & OBJECT_EMISSIVE) != 0)
    {
        // Светится изнутри: середина ярче края, ярче белого (её подхватит свечение).
        color = o.color.rgb * (0.8 + 1.6 * flat_part);
    }
    else
    {
        float diffuse = 0.55 + 0.45 * dot(normal, to_light);
        float3 half_vec = normalize(to_light + float3(0.0, 0.0, 1.0));
        float spec = pow(saturate(dot(normal, half_vec)), 40.0);
        // Как у полупрозрачной слизи: к середине цвет гуще и светлее изнутри.
        float3 body = o.color.rgb * (diffuse + 0.25 * flat_part);
        color = body + float3(1.0, 1.0, 1.0) * spec * 0.7;
    }
    return float4(color, cover * o.color.a);
}

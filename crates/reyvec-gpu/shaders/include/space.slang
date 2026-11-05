// Общее для проходов сцены «3D» (этапы 7.3–7.4): константы, луч из точки экрана, поле
// сцены и лампы. Проход «реймарчинг» (raymarch.slang) шагает лучами и пишет буфер
// геометрии, «тени 3D» (raymarch_shadow.slang) считают по нему тени и затенение в углах,
// «сцена» (raymarch_shade.slang) освещает.
//
// Луч строится так же, как Camera3D::ray в crates/reyvec-sdf/src/camera3d.rs: точка
// экрана через обратную матрицу камеры ложится на ближнюю (глубина 1) и дальнюю
// (глубина 0) плоскости. Матрица приходит столбцами. Раскладка Push — space_push в
// crates/reyvec-gpu/src/demo.rs.

#include "sdf3d.slang"

struct Push
{
    float4 inv0; // столбцы обратной матрицы «мир → экран»
    float4 inv1;
    float4 inv2;
    float4 inv3;
    float3 eye;  // где глаз (для плоской камеры не нужен: начало луча из матрицы)
    float time;  // секунды с запуска
    uint* scene; // объекты, материалы и программы форм (pack_scene)
    float aspect; // ширина / высота
    float height; // высота кадра в пикселях
    uint flags;
    uint gbuffer; // буфер геометрии в общем списке текстур (читают тени и освещение)
    uint shadows; // тени и затенение в углах (7.4, читает освещение)
    uint pad;
};
[[vk::push_constant]] ConstantBuffer<Push> pc;

struct VertexOut
{
    float4 position : SV_Position;
    float2 screen : SCREEN; // точка экрана: x и y от -1 до 1, y вверх
};

[shader("vertex")]
VertexOut vs_main(uint vertex : SV_VulkanVertexID)
{
    // Вершины (-1,-1), (3,-1), (-1,3): треугольник накрывает весь экран.
    float2 uv = float2((vertex << 1) & 2, vertex & 2);
    float2 ndc = uv * 2.0 - 1.0;
    VertexOut o;
    o.position = float4(ndc, 0.0, 1.0);
    o.screen = float2(ndc.x, -ndc.y);
    return o;
}

float3 unproject(float2 s, float depth)
{
    float4 p = pc.inv0 * s.x + pc.inv1 * s.y + pc.inv2 * depth + pc.inv3;
    return p.xyz / p.w;
}

// Луч точки экрана: начало на ближней плоскости, направление и длина до дальней.
struct Ray
{
    float3 origin;
    float3 dir;
    float limit;
};

Ray screen_ray(float2 s)
{
    float3 near = unproject(s, 1.0);
    float3 far = unproject(s, 0.0);
    Ray r;
    r.origin = near;
    r.dir = normalize(far - near);
    r.limit = length(far - near);
    return r;
}

static const uint MAX_OBJECTS = 1024;
static const uint MAX_PROGRAM_WORDS = 4096;

static const uint OBJECT_WORDS = 8; // как shape3d::OBJECT_WORDS

// Расстояние до ближайшего объекта (x) и его материал (y).
//
// У каждого конечного объекта есть шар-граница (7.6): снаружи шара до формы не ближе,
// чем до шара. Поэтому объект, чей шар дальше уже найденного, не считается вовсе (это
// точно), а объект, чей шар дальше `near`, отвечает расстоянием до шара (это меньше
// настоящего, но шагать по лучу на него безопасно). Программа формы выполняется только
// вблизи объекта. `near` выбирает вызывающий: шагание берёт больше своей точности
// попадания, чтобы попадание всегда считалось по настоящей форме; нормаль и углы —
// бесконечность (только точный пропуск).
float2 scene_near(float3 p, float near)
{
    // Числа из буфера ограничены сверху: испорченный буфер не должен вешать видеокарту.
    uint count = min(pc.scene[0], MAX_OBJECTS);
    uint objects = pc.scene[1];
    float2 best = float2(1e9, 0.0);
    for (uint o = 0; o < count; o++)
    {
        uint w = objects + o * OBJECT_WORDS;
        float radius = asfloat(pc.scene[w + 7]);
        if (radius >= 0.0)
        {
            float3 center = float3(asfloat(pc.scene[w + 4]), asfloat(pc.scene[w + 5]), asfloat(pc.scene[w + 6]));
            float outside = length(p - center) - radius;
            if (outside >= best.x)
                continue;
            if (outside > near)
            {
                best = float2(outside, 0.0);
                continue;
            }
        }
        uint start = pc.scene[w];
        uint len = min(pc.scene[w + 1], MAX_PROGRAM_WORDS);
        float2 s = sdf_program(pc.scene, start, len, p);
        if (s.x < best.x)
            best = s;
    }
    return best;
}

// Нормаль в буфере геометрии: два числа вместо трёх (октаэдрическая раскладка: сфера
// направлений разворачивается в квадрат).
float2 oct_encode(float3 n)
{
    n /= abs(n.x) + abs(n.y) + abs(n.z);
    float2 e = n.xy;
    if (n.z < 0.0)
        e = (1.0 - abs(e.yx)) * select(e >= 0.0, float2(1.0), float2(-1.0));
    return e;
}

float3 oct_decode(float2 e)
{
    float3 n = float3(e, 1.0 - abs(e.x) - abs(e.y));
    if (n.z < 0.0)
        n.xy = (1.0 - abs(n.yx)) * select(n.xy >= 0.0, float2(1.0), float2(-1.0));
    return normalize(n);
}

// Буфер геометрии одной точки экрана (см. raymarch.slang).
struct Surface
{
    float t;        // расстояние вдоль луча; меньше нуля — небо
    float material;
    float3 normal;
};

Surface read_surface(float4 g)
{
    Surface s;
    s.t = g.x;
    s.material = g.y;
    s.normal = oct_decode(g.zw);
    return s;
}

// Лампы (7.4): заголовок сцены, слова 4 и 5 — где и сколько. Запись лампы — 8 слов:
// место, дальность, цвет, яркость (Lamp3 в crates/reyvec-sdf/src/shape3d.rs).
static const uint MAX_LAMPS = 64;
static const uint SHADOW_LAMPS = 2; // как shape3d::SHADOW_LAMPS

struct Lamp
{
    float3 at;
    float range;
    float3 color;
    float power;
};

uint lamp_count()
{
    return min(pc.scene[5], MAX_LAMPS);
}

Lamp read_lamp(uint i)
{
    uint w = pc.scene[4] + i * 8;
    Lamp l;
    l.at = float3(asfloat(pc.scene[w]), asfloat(pc.scene[w + 1]), asfloat(pc.scene[w + 2]));
    l.range = asfloat(pc.scene[w + 3]);
    l.color = float3(asfloat(pc.scene[w + 4]), asfloat(pc.scene[w + 5]), asfloat(pc.scene[w + 6]));
    l.power = asfloat(pc.scene[w + 7]);
    return l;
}

// Обратный квадрат, плавно гаснущий к дальности. Зеркало shape3d::lamp_falloff.
float lamp_falloff(Lamp l, float distance)
{
    float x = min(distance / max(l.range, 1e-3), 1.0);
    float fade = max(1.0 - x * x * x * x, 0.0);
    return l.power * fade * fade / (distance * distance + 1.0);
}

// Солнце: сбоку сверху.
static const float3 SUN = float3(0.4969, 0.7950, 0.3478); // normalize(0.5, 0.8, 0.35)

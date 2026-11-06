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
    // Куда вычислительный проход пишет: номер в images[] (реймарчинг 7.6м — буфер
    // геометрии, края 7.6о — цвет краёв). Освещение читает по нему цвет краёв из textures[].
    uint target;
    uint cone; // длины начала лучей (7.8, raymarch_cone.slang)
    uint meshes; // буфер геометрии полигонов (7.6ж), если стоит SPACE_FLAG_MESHES
    uint* edges; // список пикселей на краях форм (7.6о, raymarch_edge_find.slang)
};
[[vk::push_constant]] ConstantBuffer<Push> pc;

// Треугольник на весь экран для проходов-лучей. Файл со своим вершинным шейдером
// (mesh3d.slang) определяет SPACE_OWN_VERTEX до подключения.
#ifndef SPACE_OWN_VERTEX
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
#endif

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
static const uint MAX_PROGRAM_WORDS = 16384;

static const uint OBJECT_WORDS = 8; // как shape3d::OBJECT_WORDS
static const uint OBJECT_SHADOW_ONLY = 1; // как shape3d::OBJECT_SHADOW_ONLY
static const uint OBJECT_PLANE = 2; // как shape3d::OBJECT_PLANE
static const uint OBJECT_CHAIN = 4; // как shape3d::OBJECT_CHAIN
static const uint MAX_PLANES = 8; // как shape3d::MAX_PLANES
// Флаги констант (pc.flags) сцены «3D», как SPACE_FLAG_* в demo.rs.
static const uint SPACE_FLAG_MESHES = 1; // у «реймарчинга» есть буфер геометрии полигонов
static const uint SPACE_FLAG_EDGES = 2; // края форм сглажены (7.6о): освещение берёт их цвет

// Расстояние до ближайшего объекта (x) и его материал (y).
//
// У каждого конечного объекта есть шар-граница (7.6): снаружи шара до формы не ближе,
// чем до шара. Поэтому объект, чей шар дальше уже найденного, не считается вовсе (это
// точно), а объект, чей шар дальше `near`, отвечает расстоянием до шара (это меньше
// настоящего, но шагать по лучу на него безопасно). Программа формы выполняется только
// вблизи объекта. `near` выбирает вызывающий: шагание берёт больше своей точности
// попадания, чтобы попадание всегда считалось по настоящей форме; нормаль и углы —
// бесконечность (только точный пропуск). Ответ-оценка помечен материалом -1.
//
// Сетка отсева (7.6е, crates/reyvec-sdf/src/grid3d.rs): точка берёт свою клетку и
// перебирает только её список (объекты ближе размера клетки) и бесконечные формы.
// Объекты не из списка дальше размера клетки — это начальная оценка. Снаружи сетки
// оценка — расстояние до коробки, где лежат все шары-границы. Без сетки (слово 6
// заголовка 0) перебираются все объекты. Список один проход: тело цикла встраивается
// в шейдер один раз.
//
// Заменители полигональных моделей (7.6ж, флаг OBJECT_SHADOW_ONLY) видны только при
// `proxies`: тени и углы их считают, лучи глаза и нормали — нет.
//
// Плоскости (7.6и, флаг OBJECT_PLANE: пол, стены) видны только при `planes`. Луч глаза
// находит их точку формулой (plane_hit) и шагает без них: иначе у пола каждый шаг
// короче высоты над полом, и луч, скользящий вдоль пола, тратит на это десятки шагов.
float2 scene_near(float3 p, float near, bool proxies, bool planes)
{
    // Числа из буфера ограничены сверху: испорченный буфер не должен вешать видеокарту.
    uint count = min(pc.scene[0], MAX_OBJECTS);
    uint objects = pc.scene[1];
    uint g = pc.scene[6];
    float2 best = float2(1e9, 0.0);
    // Список — два куска подряд: бесконечные формы, потом объекты клетки. Без сетки —
    // все объекты по порядку (номер — сам индекс).
    uint first = 0;
    uint first_count = count;
    uint second = 0;
    uint second_count = 0;
    bool direct = g == 0;
    if (!direct)
    {
        first = pc.scene[g + 16];
        // Плоскости — в конце списка бесконечных форм: без них список короче.
        first_count = min(pc.scene[g + (planes ? 17 : 18)], MAX_OBJECTS);
        float3 origin = float3(asfloat(pc.scene[g]), asfloat(pc.scene[g + 1]), asfloat(pc.scene[g + 2]));
        float cell = asfloat(pc.scene[g + 3]);
        int3 dims = int3(pc.scene[g + 4], pc.scene[g + 5], pc.scene[g + 6]);
        int3 q = int3(floor((p - origin) / cell));
        if (all(q >= 0) && all(q < dims))
        {
            uint c = pc.scene[g + 14] + uint((q.z * dims.y + q.y) * dims.x + q.x) * 2;
            second = pc.scene[c];
            second_count = min(pc.scene[c + 1], MAX_OBJECTS);
            best = float2(asfloat(pc.scene[g + 7]), -1.0);
        }
        else
        {
            float3 lo = float3(asfloat(pc.scene[g + 8]), asfloat(pc.scene[g + 9]), asfloat(pc.scene[g + 10]));
            float3 hi = float3(asfloat(pc.scene[g + 11]), asfloat(pc.scene[g + 12]), asfloat(pc.scene[g + 13]));
            best = float2(length(p - clamp(p, lo, hi)), -1.0);
        }
    }
    uint total = first_count + second_count;
    for (uint k = 0; k < total; k++)
    {
        uint o = direct ? k : (k < first_count ? pc.scene[first + k] : pc.scene[second + k - first_count]);
        if (o >= count)
            continue;
        uint w = objects + o * OBJECT_WORDS;
        uint flags = pc.scene[w + 2];
        if (!proxies && (flags & OBJECT_SHADOW_ONLY) != 0)
            continue;
        if (!planes && (flags & OBJECT_PLANE) != 0)
            continue;
        float radius = asfloat(pc.scene[w + 7]);
        if (radius >= 0.0)
        {
            float3 center = float3(asfloat(pc.scene[w + 4]), asfloat(pc.scene[w + 5]), asfloat(pc.scene[w + 6]));
            float outside = length(p - center) - radius;
            if (outside >= best.x)
                continue;
            if (outside > near)
            {
                best = float2(outside, -1.0);
                continue;
            }
        }
        uint start = pc.scene[w];
        uint len = min(pc.scene[w + 1], MAX_PROGRAM_WORDS);
        // Объект из одного примитива (шар, коробка, пол — их большинство) считается
        // напрямую: без стека машины, который видеокарта держит в медленной памяти (7.6в).
        float2 s;
        uint head = pc.scene[start];
        if (len == PRIM_WORDS + (head >> 16) && (head & 0xff) == OP_PRIM)
            s = sdf_primitive(pc.scene, start, p);
        else if ((flags & OBJECT_CHAIN) != 0)
            s = sdf_chain(pc.scene, start, len, p);
        else
            s = sdf_program(pc.scene, start, len, p);
        if (s.x < best.x)
            best = s;
    }
    return best;
}

// Ближайшая точка плоскостей сцены на луче (7.6и): x — длина вдоль луча (1e30 — ни
// одной плоскости впереди), y — материал; `normal` — нормаль этой плоскости. Луч,
// начатый за плоскостью (внутри), упирается в неё сразу.
float2 plane_hit(float3 origin, float3 dir, out float3 normal)
{
    uint list = pc.scene[8];
    uint count = min(pc.scene[9], MAX_PLANES);
    uint objects = pc.scene[1];
    float2 best = float2(1e30, 0.0);
    normal = float3(0.0, 1.0, 0.0);
    for (uint k = 0; k < count; k++)
    {
        uint w = objects + min(pc.scene[list + k], MAX_OBJECTS - 1) * OBJECT_WORDS;
        float3 n = float3(asfloat(pc.scene[w + 4]), asfloat(pc.scene[w + 5]), asfloat(pc.scene[w + 6]));
        float above = dot(n, origin) + asfloat(pc.scene[w + 3]);
        float toward = dot(n, dir);
        float t = above <= 0.0 ? 0.0 : (toward < 0.0 ? -above / toward : 1e30);
        if (t < best.x)
        {
            best = float2(t, float(pc.scene[pc.scene[w] + 1]));
            normal = n;
        }
    }
    return best;
}

// Уходит ли луч от всех плоскостей сцены (7.6и): тогда плоскости его не заслонят, и
// шагать по лучу тени можно без них (у пола шаг иначе не длиннее высоты над полом).
bool away_from_planes(float3 origin, float3 dir)
{
    uint list = pc.scene[8];
    uint count = min(pc.scene[9], MAX_PLANES);
    uint objects = pc.scene[1];
    for (uint k = 0; k < count; k++)
    {
        uint w = objects + min(pc.scene[list + k], MAX_OBJECTS - 1) * OBJECT_WORDS;
        float3 n = float3(asfloat(pc.scene[w + 4]), asfloat(pc.scene[w + 5]), asfloat(pc.scene[w + 6]));
        if (dot(n, dir) < 0.0 || dot(n, origin) + asfloat(pc.scene[w + 3]) <= 0.0)
            return false;
    }
    return true;
}

// Расстояние до плоскостей сцены (пол, стены), найденных формулой (7.6и): запечённое
// поле (7.10) их не хранит.
float planes_distance(float3 p)
{
    uint list = pc.scene[8];
    uint count = min(pc.scene[9], MAX_PLANES);
    uint objects = pc.scene[1];
    float best = 1e9;
    for (uint k = 0; k < count; k++)
    {
        uint w = objects + min(pc.scene[list + k], MAX_OBJECTS - 1) * OBJECT_WORDS;
        float3 n = float3(asfloat(pc.scene[w + 4]), asfloat(pc.scene[w + 5]), asfloat(pc.scene[w + 6]));
        best = min(best, dot(n, p) + asfloat(pc.scene[w + 3]));
    }
    return best;
}

#include "field3d.slang"

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
static const float3 SUN = float3(0.4969, 0.7950, 0.3478); // normalize(0.5, 0.8, 0.35); как view3d::SUN

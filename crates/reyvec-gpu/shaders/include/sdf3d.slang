// Объёмные формы (этап 7.2): примитивы, операции и стековая машина, которая выполняет
// программу формы. Зеркало crates/reyvec-sdf/src/shape3d.rs (Program::distance) — те же
// формулы и та же раскладка слов, менять вместе.
//
// Программа — слова подряд. Примитив (19 слов): команда (код 1 и вид в битах 8–15),
// материал, матрица «мир → форма» тремя строками по 4 числа (поворот и сдвиг), множитель
// расстояния (масштаб), 4 параметра. Биты 16–31 команды — сколько слов данных идёт после
// примитива: поле 2D-фигуры у выдавливания и вращения (7.7, раскладка — в
// crates/reyvec-sdf/src/profile2d.rs). Операция (2 слова): код (2 — объединить, 3 —
// вырезать, 4 — пересечь) и ширина плавного перехода.

#include "textures.slang"

static const uint OP_PRIM = 1;
static const uint OP_UNION = 2;
static const uint OP_SUBTRACT = 3;
static const uint OP_INTERSECT = 4;

static const uint PRIM_SPHERE = 0;
static const uint PRIM_BOX = 1;
static const uint PRIM_CAPSULE = 2;
static const uint PRIM_CYLINDER = 3;
static const uint PRIM_TORUS = 4;
static const uint PRIM_PLANE = 5;
static const uint PRIM_EXTRUDE = 6;
static const uint PRIM_REVOLVE = 7;

static const uint PRIM_WORDS = 19;
static const uint FIELD_HEADER_WORDS = 8;
static const uint OP_WORDS = 2;
static const uint SDF_STACK = 8;

float sd3_sphere(float3 p, float r)
{
    return length(p) - r;
}

float sd3_box(float3 p, float3 half_size, float round)
{
    float3 q = abs(p) - half_size + round;
    return length(max(q, 0.0)) + min(max(q.x, max(q.y, q.z)), 0.0) - round;
}

float sd3_capsule(float3 p, float half_height, float r)
{
    float y = clamp(p.y, -half_height, half_height);
    return length(p - float3(0.0, y, 0.0)) - r;
}

float sd3_cylinder(float3 p, float half_height, float r, float round)
{
    float2 d = float2(length(p.xz) - r + round, abs(p.y) - half_height + round);
    return length(max(d, 0.0)) + min(max(d.x, d.y), 0.0) - round;
}

float sd3_torus(float3 p, float big, float small)
{
    return length(float2(length(p.xz) - big, p.y)) - small;
}

float sd3_plane(float3 p, float3 n, float offset)
{
    return dot(p, n) - offset;
}

// Расстояние до 2D-фигуры по её запечённому полю с слова `at` (зеркало field_distance
// в profile2d.rs): внутри сетки — смесь четырёх соседних точек, снаружи — оценка снизу
// от края сетки и от прямоугольника фигуры.
float sd2_field(uint* code, uint at, float2 p)
{
    float2 corner = float2(asfloat(code[at]), asfloat(code[at + 1]));
    float cell = asfloat(code[at + 2]);
    uint nx = code[at + 3] & 0xffff;
    uint ny = code[at + 3] >> 16;
    float2 g = (p - corner) / cell - 0.5;
    float2 top = float2(float(nx) - 1.0, float(ny) - 1.0);
    float2 gc = clamp(g, float2(0.0, 0.0), top);
    float e = length(g - gc) * cell;
    float2 i0 = min(floor(gc), top - 1.0);
    float2 t = gc - i0;
    uint base = at + FIELD_HEADER_WORDS + uint(i0.y) * nx + uint(i0.x);
    float v = lerp(lerp(asfloat(code[base]), asfloat(code[base + 1]), t.x),
                   lerp(asfloat(code[base + nx]), asfloat(code[base + nx + 1]), t.x), t.y);
    if (e > 0.0)
    {
        float2 lo = float2(asfloat(code[at + 4]), asfloat(code[at + 5]));
        float2 hi = float2(asfloat(code[at + 6]), asfloat(code[at + 7]));
        float to_box = length(max(max(lo - p, p - hi), 0.0));
        float vp = max(v, 0.0);
        return max(sqrt(e * e + vp * vp), to_box);
    }
    return v;
}

// Выдавливание: d2 — расстояние до 2D-фигуры в точке (x, y), толщина ±half по Z.
float sd3_extrude(float3 p, float d2, float half_depth, float round)
{
    float2 w = float2(d2 + round, abs(p.z) - half_depth + round);
    return length(max(w, 0.0)) + min(max(w.x, w.y), 0.0) - round;
}

float op3_union(float a, float b, float k)
{
    if (k <= 0.0)
        return min(a, b);
    float h = saturate(0.5 + 0.5 * (b - a) / k);
    return lerp(b, a, h) - k * h * (1.0 - h);
}

float op3_subtract(float a, float b, float k)
{
    return -op3_union(-a, b, k);
}

float op3_intersect(float a, float b, float k)
{
    return -op3_union(-a, -b, k);
}

// Один примитив, записанный с слова `i`: расстояние (уже с масштабом) и материал.
float2 sdf_primitive(uint* code, uint i, float3 p)
{
    uint kind = (code[i] >> 8) & 0xff;
    float material = float(code[i + 1]);
    float4 r0 = float4(asfloat(code[i + 2]), asfloat(code[i + 3]), asfloat(code[i + 4]), asfloat(code[i + 5]));
    float4 r1 = float4(asfloat(code[i + 6]), asfloat(code[i + 7]), asfloat(code[i + 8]), asfloat(code[i + 9]));
    float4 r2 = float4(asfloat(code[i + 10]), asfloat(code[i + 11]), asfloat(code[i + 12]), asfloat(code[i + 13]));
    float3 q = float3(dot(r0.xyz, p) + r0.w, dot(r1.xyz, p) + r1.w, dot(r2.xyz, p) + r2.w);
    float scale = asfloat(code[i + 14]);
    float4 a = float4(asfloat(code[i + 15]), asfloat(code[i + 16]), asfloat(code[i + 17]), asfloat(code[i + 18]));
    float d;
    if (kind == PRIM_SPHERE)
        d = sd3_sphere(q, a.x);
    else if (kind == PRIM_BOX)
        d = sd3_box(q, a.xyz, a.w);
    else if (kind == PRIM_CAPSULE)
        d = sd3_capsule(q, a.x, a.y);
    else if (kind == PRIM_CYLINDER)
        d = sd3_cylinder(q, a.x, a.y, a.z);
    else if (kind == PRIM_TORUS)
        d = sd3_torus(q, a.x, a.y);
    else if (kind == PRIM_EXTRUDE)
        d = sd3_extrude(q, sd2_field(code, i + PRIM_WORDS, q.xy), a.x, a.y);
    else if (kind == PRIM_REVOLVE)
        d = sd2_field(code, i + PRIM_WORDS, float2(length(q.xz) - a.x, q.y));
    else
        d = sd3_plane(q, a.xyz, a.w);
    return float2(d * scale, material);
}

// Расстояние до формы (x) и материал ближайшей части (y, номер как float) по программе
// из `len` слов, начиная с `start`.
float2 sdf_program(uint* code, uint start, uint len, float3 p)
{
    float2 stack[SDF_STACK];
    int top = 0;
    uint i = start;
    uint end = start + len;
    while (i < end)
    {
        uint word = code[i];
        uint op = word & 0xff;
        if (op == OP_PRIM)
        {
            float2 s = sdf_primitive(code, i, p);
            if (top < int(SDF_STACK))
                stack[top] = s;
            top++;
            i += PRIM_WORDS + (word >> 16);
        }
        else
        {
            // Неизвестная команда — испорченная программа: дальше не читаем.
            if (op < OP_UNION || op > OP_INTERSECT)
                break;
            float k = asfloat(code[i + 1]);
            top--;
            float2 b = stack[max(top, 0)];
            float2 a = stack[max(top - 1, 0)];
            float d;
            if (op == OP_UNION)
                d = op3_union(a.x, b.x, k);
            else if (op == OP_SUBTRACT)
                d = op3_subtract(a.x, b.x, k);
            else
                d = op3_intersect(a.x, b.x, k);
            // Материал: у объединения — ближайшей части, иначе — основы.
            float material = (op == OP_UNION && b.x < a.x) ? b.y : a.y;
            stack[max(top - 1, 0)] = float2(d, material);
            i += OP_WORDS;
        }
    }
    return top > 0 ? stack[0] : float2(1e9, 0.0);
}

// То же для программы, где ждёт операции не больше одной формы (7.6т): цепочка «форма,
// форма, операция, форма, операция…» — объединение и пересечение списком, вырезание.
// Таких почти все. Стек машины видеокарта держит в медленной памяти, а здесь он не нужен:
// хватает итога `acc` и следующей формы `next` в регистрах. Признак — флаг объекта
// OBJECT_CHAIN, его ставит упаковка сцены на процессоре (shape3d::Program::chain).
float2 sdf_chain(uint* code, uint start, uint len, float3 p)
{
    float2 acc = float2(1e9, 0.0);
    float2 next = float2(1e9, 0.0);
    bool first = true;
    uint i = start;
    uint end = start + len;
    while (i < end)
    {
        uint word = code[i];
        uint op = word & 0xff;
        if (op == OP_PRIM)
        {
            float2 s = sdf_primitive(code, i, p);
            if (first)
                acc = s;
            else
                next = s;
            first = false;
            i += PRIM_WORDS + (word >> 16);
        }
        else
        {
            if (op < OP_UNION || op > OP_INTERSECT)
                break;
            float k = asfloat(code[i + 1]);
            float d;
            if (op == OP_UNION)
                d = op3_union(acc.x, next.x, k);
            else if (op == OP_SUBTRACT)
                d = op3_subtract(acc.x, next.x, k);
            else
                d = op3_intersect(acc.x, next.x, k);
            float material = (op == OP_UNION && next.x < acc.x) ? next.y : acc.y;
            acc = float2(d, material);
            i += OP_WORDS;
        }
    }
    return acc;
}

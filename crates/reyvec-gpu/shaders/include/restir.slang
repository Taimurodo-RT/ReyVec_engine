// ReSTIR DI (путь ReSTIR, шаг 3): выбор точки света с переиспользованием. Перед
// подключением — light3d.slang.
//
// У каждого пикселя — «запас» (reservoir): одна выбранная точка света (номер источника и
// два числа, light3d.slang; солнце — отдельно, лучом всегда), её вес W и сколько кандидатов за ней стоит (M). Пиксель сам
// пробует несколько случайных точек, потом берёт запас своего места в прошлом кадре и
// запасы соседей: выбор по весам «сколько света дала бы эта точка здесь». В итоге из
// тысяч возможных точек каждый пиксель проверяет лучом тени одну — самую «весомую», а
// шума почти как у сотен проб.
//
// Буфер запасов (адрес — pc.edges): два слоя по 8 слов на пиксель. Слой 0 — итог
// прошлого кадра (его читает проход «ReSTIR: кандидаты» и переписывает проход «путь 3D»),
// слой 1 — запасы этого кадра до соседей. Запись: номер источника (~0 — пусто), два
// числа (по 16 бит), W, M, точка поверхности (3 числа) и нормаль (октаэдр, по 16 бит).
//
// Смещение без поправок (biased ReSTIR): соседи и прошлый кадр берутся, только если
// поверхность почти та же. Шумодав (шаг 5) — отдельно.

static const uint RESTIR_WORDS = 8;
// Сколько случайных точек пробует пиксель сам.
static const uint RESTIR_CANDIDATES = 32;
// Прошлый кадр весит не больше стольких «своих» проб: свет меняется — запас обновляется.
static const float RESTIR_HISTORY = 20.0;
static const uint RESTIR_EMPTY = 0xffffffffu;

struct Reservoir
{
    uint light;
    float2 uv;
    float W;
    float M;
    float3 pos;
    float3 normal;
};

Reservoir reservoir_empty()
{
    Reservoir r;
    r.light = RESTIR_EMPTY;
    r.uv = float2(0.0, 0.0);
    r.W = 0.0;
    r.M = 0.0;
    r.pos = float3(0.0, 0.0, 0.0);
    r.normal = float3(0.0, 1.0, 0.0);
    return r;
}

uint pack_unorm2(float2 v)
{
    uint2 q = uint2(saturate(v) * 65535.0 + 0.5);
    return q.x | (q.y << 16);
}

float2 unpack_unorm2(uint w)
{
    return float2(float(w & 0xffffu), float(w >> 16)) / 65535.0;
}

uint restir_at(uint layer, int2 px, uint w, uint h)
{
    return ((layer * h + uint(px.y)) * w + uint(px.x)) * RESTIR_WORDS;
}

Reservoir reservoir_load(uint layer, int2 px, uint w, uint h)
{
    uint at = restir_at(layer, px, w, h);
    Reservoir r;
    r.light = pc.edges[at];
    r.uv = unpack_unorm2(pc.edges[at + 1]);
    r.W = asfloat(pc.edges[at + 2]);
    r.M = asfloat(pc.edges[at + 3]);
    r.pos = float3(asfloat(pc.edges[at + 4]), asfloat(pc.edges[at + 5]), asfloat(pc.edges[at + 6]));
    r.normal = oct_decode(unpack_unorm2(pc.edges[at + 7]) * 2.0 - 1.0);
    return r;
}

void reservoir_store(uint layer, int2 px, uint w, uint h, Reservoir r)
{
    uint at = restir_at(layer, px, w, h);
    pc.edges[at] = r.light;
    pc.edges[at + 1] = pack_unorm2(r.uv);
    pc.edges[at + 2] = asuint(r.W);
    pc.edges[at + 3] = asuint(r.M);
    pc.edges[at + 4] = asuint(r.pos.x);
    pc.edges[at + 5] = asuint(r.pos.y);
    pc.edges[at + 6] = asuint(r.pos.z);
    pc.edges[at + 7] = pack_unorm2(oct_encode(r.normal) * 0.5 + 0.5);
}

// Сборка запаса: сумма весов и «сколько света» у выбранной точки здесь.
struct ReservoirBuilder
{
    Reservoir r;
    float wsum;
    float target;
};

ReservoirBuilder builder_new(float3 pos, float3 normal)
{
    ReservoirBuilder b;
    b.r = reservoir_empty();
    b.r.pos = pos;
    b.r.normal = normal;
    b.wsum = 0.0;
    b.target = 0.0;
    return b;
}

// Добавить точку `light`/`uv` с весом `weight` (её «свет здесь» — `target`) за `m`
// кандидатов.
void builder_add(inout ReservoirBuilder b, uint light, float2 uv, float target, float weight, float m)
{
    b.r.M += m;
    if (weight <= 0.0)
        return;
    b.wsum += weight;
    if (rnd() * b.wsum <= weight)
    {
        b.r.light = light;
        b.r.uv = uv;
        b.target = target;
    }
}

// Готовый запас: W = сумма весов / (M × свет выбранной точки).
Reservoir builder_finish(ReservoirBuilder b)
{
    Reservoir r = b.r;
    r.W = (b.target > 0.0 && r.M > 0.0) ? b.wsum / (r.M * b.target) : 0.0;
    if (r.W <= 0.0)
        r.light = RESTIR_EMPTY;
    return r;
}

// Взять чужой запас `other` (соседа или прошлого кадра), если его поверхность почти та же,
// что у точки `pos` с нормалью `normal` (`t` — длина луча глаза до неё). `q` — точка над
// поверхностью, `dir` — луч глаза, `sm` — материал. `cap` — предел M.
void builder_merge(inout ReservoirBuilder b, Reservoir other, float cap, float3 q, float3 dir,
                   SurfaceMaterial sm, float t)
{
    if (other.M <= 0.0)
        return;
    if (abs(dot(other.pos - b.r.pos, b.r.normal)) > 0.02 + 0.01 * t || dot(other.normal, b.r.normal) < 0.9)
        return;
    float m = min(other.M, cap);
    if (other.light == RESTIR_EMPTY || other.W <= 0.0)
    {
        b.r.M += m;
        return;
    }
    LightSample s = light_sample(other.light, other.uv, q, b.r.normal, dir, sm);
    float target = luminance(s.light);
    // Точка, которая здесь не светит (лампа за поверхностью или дальше своей дальности),
    // не считается и в M: иначе чужие кандидаты только разбавляют свои, и картинка темнеет
    // (смещение «простого» ReSTIR вдали, где сосед — уже в метрах).
    if (target <= 0.0)
        return;
    builder_add(b, other.light, other.uv, target, target * other.W * m, m);
}

// Что из прошлого кадра годится (слово после матрицы камеры прошлого кадра): запасы ReSTIR
// и память шумодава, запасы ReSTIR GI (как PREV_* в path_ref.rs).
static const uint PREV_RESTIR = 1;
static const uint PREV_HISTORY = 2;
static const uint PREV_GI = 4;
// Камера не сдвинулась с прошлого кадра (та же матрица).
static const uint PREV_STILL = 8;

// Камера та же, что в прошлом кадре.
bool camera_still()
{
    uint at = pc.scene[7];
    return at != 0u && (pc.scene[at + MESH_PREV_VIEW_PROJ + 16] & PREV_STILL) != 0u;
}

// Пиксель прошлого кадра для точки `p` (матрица камеры прошлого кадра — в разделе моделей);
// false — прошлого кадра нет, его данные `what` не годятся или точка вне него.
bool previous_pixel(float3 p, uint w, uint h, uint what, out int2 px)
{
    px = int2(-1, -1);
    uint at = pc.scene[7];
    if (at == 0u || (pc.scene[at + MESH_PREV_VIEW_PROJ + 16] & what) == 0u)
        return false;
    uint m = at + MESH_PREV_VIEW_PROJ;
    float4 clip = float4(0.0, 0.0, 0.0, 0.0);
    for (uint c = 0; c < 4; c++)
    {
        float4 column = float4(asfloat(pc.scene[m + 4 * c]), asfloat(pc.scene[m + 4 * c + 1]),
                               asfloat(pc.scene[m + 4 * c + 2]), asfloat(pc.scene[m + 4 * c + 3]));
        clip += column * (c < 3 ? p[c] : 1.0);
    }
    if (clip.w <= 1e-6)
        return false;
    float2 s = clip.xy / clip.w;
    float2 f = float2((s.x + 1.0) * 0.5 * float(w), (1.0 - s.y) * 0.5 * float(h));
    px = int2(floor(f));
    return all(px >= 0) && px.x < int(w) && px.y < int(h);
}

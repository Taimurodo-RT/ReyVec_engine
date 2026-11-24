// ReSTIR GI (путь ReSTIR, шаг 4): отражённый свет с переиспользованием. Перед подключением —
// restir.slang.
//
// Отражённый (непрямой) свет точки — это свет, пришедший от других поверхностей. Путь
// пикселя после первого рассеянного отскока попадает в точку x_s и приносит её свет L_s:
// это «кандидат». Запас ReSTIR GI хранит одну такую точку: где она (и её нормаль), сколько
// света она шлёт, вес W и сколько кандидатов за ней стоит (M). Как и у ReSTIR DI, пиксель
// берёт запас своего места в прошлом кадре и запасы соседей: точки, которые сосед уже
// нашёл, светят и сюда. Один путь на пиксель — а отражённый свет почти как у десятков.
//
// Буфер — тот же, что у запасов DI (pc.edges), после двух их слоёв: два слоя по GI_WORDS
// слов на пиксель. Слой 0 — итог кадра (пишет проход «ReSTIR GI: соседи», читает «путь
// 3D» следующего кадра), слой 1 — свой кандидат пикселя и прошлый кадр (пишет «путь 3D»,
// соседи берут его в «ReSTIR GI: соседи»). Запись: точка (3 числа), нормаль (октаэдр, по 16
// бит), свет (общая экспонента, 9+9+9+5 бит), W, M, точка пикселя (3 числа) и её нормаль.
//
// Чужую точку x_s пиксель видит под другим углом и с другого расстояния: вес умножается на
// поправку (якобиан) cos_q·d_r² / (cos_r·d_q²); слишком большая поправка — точка не
// берётся. Луч видимости к выбранной точке — один на пиксель (в «соседях»).

static const uint GI_WORDS = 11;
// Прошлый кадр весит не больше стольких кандидатов: свет меняется — запас обновляется.
static const float GI_HISTORY = 30.0;
static const uint GI_NEIGHBORS = 3;
static const float GI_RADIUS = 24.0;
// Точка неба (путь ушёл мимо всего) — на таком расстоянии по лучу.
static const float GI_SKY = 80.0;
// Предел яркости света кандидата: редкий яркий путь (светлячок) иначе раздаётся соседям
// и следующим кадрам пятнами. Небольшое смещение у самых ярких мест — плата за это.
static const float GI_CLAMP = 4.0;
// Поправку на угол и расстояние больше стольких раз (и меньше обратного) не берём.
static const float GI_JACOBIAN = 3.0;

// Материал первой точки по слову буфера геометрии.
uint gi_material(float y)
{
    return min(uint(y + 0.5), max(pc.scene[3], 1) - 1);
}

// Зеркальное (блестит и почти не шероховато) — без ReSTIR GI: его рассеянная доля мала, а
// путь, ушедший в неё, оставил бы пиксель зеркала без отражения (тёмные точки). У зеркал
// путь считается как без GI.
bool gi_mirror(uint m)
{
    return material_shine(m) > 0.3 && material_rough(m) < 0.1;
}

struct GiReservoir
{
    float3 pos;
    float3 normal;
    float3 radiance;
    float W;
    float M;
    // Точка пикселя, для которой собран запас, и её нормаль.
    float3 at;
    float3 at_normal;
};

GiReservoir gi_empty(float3 at, float3 at_normal)
{
    GiReservoir r;
    r.pos = float3(0.0, 0.0, 0.0);
    r.normal = float3(0.0, 1.0, 0.0);
    r.radiance = float3(0.0, 0.0, 0.0);
    r.W = 0.0;
    r.M = 0.0;
    r.at = at;
    r.at_normal = at_normal;
    return r;
}

// Цвет в одно слово: три мантиссы по 9 бит и общая экспонента (как формат RGB9E5).
uint pack_rgb9e5(float3 c)
{
    c = clamp(c, 0.0, 65000.0);
    float m = max(c.x, max(c.y, c.z));
    if (m < 1e-9)
        return 0u;
    int e = clamp(int(floor(log2(m))) + 1, -15, 16);
    float s = exp2(float(e - 9));
    uint3 q = uint3(min(c / s + 0.5, 511.0));
    return q.x | (q.y << 9) | (q.z << 18) | (uint(e + 15) << 27);
}

float3 unpack_rgb9e5(uint w)
{
    int e = int(w >> 27) - 15;
    return float3(float(w & 511u), float((w >> 9) & 511u), float((w >> 18) & 511u)) * exp2(float(e - 9));
}

uint gi_at(uint layer, int2 px, uint w, uint h)
{
    return 2u * w * h * RESTIR_WORDS + ((layer * h + uint(px.y)) * w + uint(px.x)) * GI_WORDS;
}

GiReservoir gi_load(uint layer, int2 px, uint w, uint h)
{
    uint at = gi_at(layer, px, w, h);
    GiReservoir r;
    r.pos = float3(asfloat(pc.edges[at]), asfloat(pc.edges[at + 1]), asfloat(pc.edges[at + 2]));
    r.normal = oct_decode(unpack_unorm2(pc.edges[at + 3]) * 2.0 - 1.0);
    r.radiance = unpack_rgb9e5(pc.edges[at + 4]);
    r.W = asfloat(pc.edges[at + 5]);
    r.M = asfloat(pc.edges[at + 6]);
    r.at = float3(asfloat(pc.edges[at + 7]), asfloat(pc.edges[at + 8]), asfloat(pc.edges[at + 9]));
    r.at_normal = oct_decode(unpack_unorm2(pc.edges[at + 10]) * 2.0 - 1.0);
    return r;
}

void gi_store(uint layer, int2 px, uint w, uint h, GiReservoir r)
{
    uint at = gi_at(layer, px, w, h);
    pc.edges[at] = asuint(r.pos.x);
    pc.edges[at + 1] = asuint(r.pos.y);
    pc.edges[at + 2] = asuint(r.pos.z);
    pc.edges[at + 3] = pack_unorm2(oct_encode(r.normal) * 0.5 + 0.5);
    pc.edges[at + 4] = pack_rgb9e5(r.radiance);
    pc.edges[at + 5] = asuint(r.W);
    pc.edges[at + 6] = asuint(r.M);
    pc.edges[at + 7] = asuint(r.at.x);
    pc.edges[at + 8] = asuint(r.at.y);
    pc.edges[at + 9] = asuint(r.at.z);
    pc.edges[at + 10] = pack_unorm2(oct_encode(r.at_normal) * 0.5 + 0.5);
}

// «Сколько света дала бы» точка `pos` со светом `radiance` в точку `q` с нормалью `n`:
// яркость × косинус (рассеянная поверхность).
float gi_target(float3 radiance, float3 pos, float3 q, float3 n)
{
    float3 to = pos - q;
    float d = length(to);
    if (d < 1e-4)
        return 0.0;
    return luminance(radiance) * max(dot(n, to / d), 0.0);
}

struct GiBuilder
{
    GiReservoir r;
    float wsum;
    float target;
};

GiBuilder gi_builder(float3 at, float3 at_normal)
{
    GiBuilder b;
    b.r = gi_empty(at, at_normal);
    b.wsum = 0.0;
    b.target = 0.0;
    return b;
}

void gi_add(inout GiBuilder b, float3 pos, float3 normal, float3 radiance, float target, float weight, float m)
{
    b.r.M += m;
    if (weight <= 0.0)
        return;
    b.wsum += weight;
    if (rnd() * b.wsum <= weight)
    {
        b.r.pos = pos;
        b.r.normal = normal;
        b.r.radiance = radiance;
        b.target = target;
    }
}

// Свой кандидат: точка, в которую пришёл путь по косинусу (плотность cos/π), поэтому вес —
// π × яркость её света.
void gi_add_own(inout GiBuilder b, float3 pos, float3 normal, float3 radiance, float3 q)
{
    float l = luminance(radiance);
    if (l > GI_CLAMP)
        radiance *= GI_CLAMP / l;
    float target = gi_target(radiance, pos, q, b.r.at_normal);
    gi_add(b, pos, normal, radiance, target, PI * luminance(radiance), 1.0);
}

// Взять чужой запас `other`, если его точка пикселя почти та же поверхность (`t` — длина
// луча глаза). `q` — точка над поверхностью этого пикселя, `cap` — предел M.
void gi_merge(inout GiBuilder b, GiReservoir other, float cap, float3 q, float t)
{
    if (other.M <= 0.0)
        return;
    float3 n = b.r.at_normal;
    if (abs(dot(other.at - b.r.at, n)) > 0.02 + 0.01 * t || dot(other.at_normal, n) < 0.9)
        return;
    float m = min(other.M, cap);
    if (other.W <= 0.0)
    {
        b.r.M += m;
        return;
    }
    float target = gi_target(other.radiance, other.pos, q, n);
    if (target <= 0.0)
        return;
    // Поправка на другой угол и расстояние к точке x_s.
    float3 to_q = q - other.pos;
    float3 to_r = other.at - other.pos;
    float dq = dot(to_q, to_q), dr = dot(to_r, to_r);
    float cos_q = abs(dot(other.normal, to_q * rsqrt(max(dq, 1e-8))));
    float cos_r = abs(dot(other.normal, to_r * rsqrt(max(dr, 1e-8))));
    float jacobian = (cos_q * dr) / max(cos_r * dq, 1e-8);
    if (cos_r < 1e-3 || jacobian * GI_JACOBIAN < 1.0 || jacobian > GI_JACOBIAN)
        return;
    gi_add(b, other.pos, other.normal, other.radiance, target, target * other.W * m * jacobian, m);
}

GiReservoir gi_finish(GiBuilder b)
{
    GiReservoir r = b.r;
    r.W = (b.target > 0.0 && r.M > 0.0) ? b.wsum / (r.M * b.target) : 0.0;
    return r;
}

// Отражённый свет запаса `r` в точке `q` с нормалью `n`, без цвета поверхности (его
// умножит «шумодав: сборка»): L × cos / π × W.
float3 gi_light(GiReservoir r, float3 q, float3 n)
{
    float3 to = r.pos - q;
    float d = length(to);
    if (r.W <= 0.0 || d < 1e-4)
        return float3(0.0, 0.0, 0.0);
    return r.radiance * (max(dot(n, to / d), 0.0) / PI * r.W);
}

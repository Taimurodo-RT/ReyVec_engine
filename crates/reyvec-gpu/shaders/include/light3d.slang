// Общее для путей света сцены «3D» (путь ReSTIR): случайные числа, направления и точки
// света солнца и ламп. Подключают raymarch_path.slang (эталон и режим «качество») и
// raymarch_restir.slang (ReSTIR DI). Перед подключением — shade3d.slang и trace3d.slang.
//
// Точка света задаётся «в пространстве света»: номер источника (0 — солнце, 1… — лампы)
// и два числа 0…1. Это пространство одно на все пиксели, поэтому выбранную точку один
// пиксель может отдать соседу или следующему кадру (ReSTIR) без пересчёта плотностей.

static const float PI = 3.14159265;
// Угловой радиус диска солнца, рад: полутень того же размера, что у мягких теней быстрого
// режима (k = 12 в shadow_part).
static const float SUN_RADIUS = 0.04;
// Радиус светящегося шарика лампы для полутени, м.
static const float LAMP_RADIUS = 0.1;
static const float3 SKY_TOP = float3(0.25, 0.30, 0.40);
static const float3 SUN_COLOR = float3(1.0, 0.95, 0.85);

static uint g_rng;

uint hash(uint x)
{
    x ^= x >> 16;
    x *= 0x7feb352d;
    x ^= x >> 15;
    x *= 0x846ca68b;
    x ^= x >> 16;
    return x;
}

// Начальное состояние случайных чисел пикселя `px` в кадре `frame` для прохода `pass`.
void seed_rng(uint2 px, uint frame, uint pass)
{
    g_rng = hash(px.x * 1973u + px.y * 9277u + hash(frame) * 26699u + pass * 104729u) | 1u;
}

float rnd()
{
    g_rng = g_rng * 747796405u + 2891336453u;
    uint w = ((g_rng >> ((g_rng >> 28u) + 4u)) ^ g_rng) * 277803737u;
    w = (w >> 22u) ^ w;
    return float(w) * (1.0 / 4294967296.0);
}

// Две оси, перпендикулярные `n`.
void basis(float3 n, out float3 t, out float3 b)
{
    float3 up = abs(n.y) < 0.99 ? float3(0.0, 1.0, 0.0) : float3(1.0, 0.0, 0.0);
    t = normalize(cross(up, n));
    b = cross(n, t);
}

// Направление вокруг `n` с плотностью по косинусу.
float3 cosine_dir(float3 n)
{
    float3 t, b;
    basis(n, t, b);
    float u = rnd(), v = rnd();
    float r = sqrt(u);
    float phi = 2.0 * PI * v;
    return normalize(t * (r * cos(phi)) + b * (r * sin(phi)) + n * sqrt(max(1.0 - u, 0.0)));
}

// Равномерно в конусе вокруг `axis` с половинным углом `angle`; `uv` — два числа 0…1.
float3 cone_dir_uv(float3 axis, float angle, float2 uv)
{
    float3 t, b;
    basis(axis, t, b);
    float c = 1.0 - uv.x * (1.0 - cos(angle));
    float s = sqrt(max(1.0 - c * c, 0.0));
    float phi = 2.0 * PI * uv.y;
    return normalize(t * (s * cos(phi)) + b * (s * sin(phi)) + axis * c);
}

float3 cone_dir(float3 axis, float angle)
{
    return cone_dir_uv(axis, angle, float2(rnd(), rnd()));
}

// Свет неба для луча, ушедшего мимо всего.
float3 sky_light(float3 dir)
{
    return dir.y >= 0.0 ? SKY_TOP : GROUND;
}

float luminance(float3 c)
{
    return dot(c, float3(0.2126, 0.7152, 0.0722));
}

bool sun_on()
{
    return (pc.scene[11] & SCENE_NO_SUN) == 0;
}

// Номер лампы-источника (1…) по числу 0…1, равновероятно. Солнце в выборе не
// участвует: оно одно, луч к нему берётся всегда (sun_light), а свет без учёта тени у
// него сильнее ламп — выбор тянулся бы к солнцу и там, где оно закрыто крышей.
uint pick_lamp(float u)
{
    uint total = lamp_count();
    return min(uint(u * float(total)), max(total, 1u) - 1u) + 1u;
}

// Точка света: откуда тень (направление и длина луча тени, 0 — света нет) и сколько света
// она даёт в точку `q` (поднятую над поверхностью) с нормалью `n` без учёта тени — как в
// shade_surface: альбедо × освещённость плюс блик. Луч пришёл в точку по `dir`.
struct LightSample
{
    float3 to;
    float far;
    float3 light;
};

LightSample light_sample(uint light, float2 uv, float3 q, float3 n, float3 dir, SurfaceMaterial sm)
{
    LightSample s;
    s.to = float3(0.0, 1.0, 0.0);
    s.far = 0.0;
    s.light = float3(0.0, 0.0, 0.0);
    if (light == 0u)
    {
        float3 l = cone_dir_uv(SUN, SUN_RADIUS, uv);
        float c = dot(n, l);
        if (c <= 0.0)
            return s;
        s.to = l;
        s.far = 30.0;
        s.light = sm.albedo * SUN_COLOR * c;
        if (!sm.grid_on)
            s.light += 0.25 * pow(saturate(dot(n, normalize(SUN - dir))), 48.0);
        return s;
    }
    Lamp lamp = read_lamp(light - 1u);
    float3 to = lamp.at - q;
    float distance = length(to);
    float3 ldir = to / max(distance, 1e-4);
    float amount = lamp_falloff(lamp, distance) * saturate(dot(n, ldir));
    if (amount <= 0.0)
        return s;
    // Точка на шарике лампы, на стороне к точке `q`. Луч тени не доходит до неё
    // LAMP_SHADOW_GAP, как в shadow_part (лампочка не закрывает свою лампу).
    float z = 1.0 - 2.0 * uv.x;
    float r = sqrt(max(1.0 - z * z, 0.0));
    float phi = 2.0 * PI * uv.y;
    float3 on = float3(r * cos(phi), r * sin(phi), z);
    if (dot(on, ldir) > 0.0)
        on = -on;
    float3 aim = lamp.at + on * LAMP_RADIUS - q;
    float length_aim = max(length(aim), 1e-4);
    s.to = aim / length_aim;
    s.far = max(length_aim - LAMP_SHADOW_GAP, 0.0);
    s.light = sm.albedo * lamp.color * amount;
    if (!sm.grid_on)
        s.light += 0.25 * amount * pow(saturate(dot(n, normalize(ldir - dir))), 48.0);
    return s;
}

// Свет солнца в точке с лучом тени к случайной точке диска.
float3 sun_light(float3 q, float3 n, float3 dir, SurfaceMaterial sm);

// Свет точки с тенью: луч тени, если он нужен.
float3 light_seen(LightSample s, float3 q)
{
    if (all(s.light <= 0.0))
        return float3(0.0, 0.0, 0.0);
    if (s.far > 0.0 && occluded(q, s.to, s.far))
        return float3(0.0, 0.0, 0.0);
    return s.light;
}

// Кэш солнца в пути (R5е): ставит «путь 3D», если карта неподвижного готова (номер
// картинки — в старших 16 битах pc.flags, как у «теней 3D»). Карта знает точно, что
// неподвижное не затеняет точку (луч тени шагает только по движущимся) или что точка в
// полной тени неподвижного (без луча: в закрытом зале — под крышей везде). Иначе — живой
// луч по всем предметам. Эталон кэш не берёт.
static bool g_sun_static = false;

float3 sun_light(float3 q, float3 n, float3 dir, SurfaceMaterial sm)
{
    if (!sun_on())
        return float3(0.0, 0.0, 0.0);
    LightSample s = light_sample(0u, float2(rnd(), rnd()), q, n, dir, sm);
    if (g_sun_static && any(s.light > 0.0))
    {
        int known = sun_static_known(q);
        if (known == 0)
            return float3(0.0, 0.0, 0.0);
        g_skip_still = known > 0;
        float3 light = light_seen(s, q);
        g_skip_still = false;
        return light;
    }
    return light_seen(s, q);
}

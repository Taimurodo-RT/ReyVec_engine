// Сцена «3D», проход «эталон 3D» (путь ReSTIR, шаг 1, docs/rays-further.md): честный
// свет без упрощений — трассировка путей (path tracing) с накоплением. Медленно и с
// шумом, но пока камера и сцена стоят, каждый кадр добавляет по пути на пиксель, и
// картинка сходится к правде. С ним сверяются быстрые режимы и будущий ReSTIR.
//
// Путь пикселя: луч глаза через случайную точку пикселя (так же сглаживаются края),
// дальше до PATH_BOUNCES отскоков. В каждой точке:
// - свечение материала светит само;
// - прямой свет с долей рассеянного (1 − доля Френеля): явный луч к солнцу (случайная
//   точка диска) и к каждой лампе (случайная точка шарика), лучи тени «да или нет»;
// - дальше с долей Френеля (блеск, reflect_share) путь отражается зеркально, у
//   шероховатых — в случайную сторону конуса (как пятно размытия Л2), иначе — отскок по
//   косинусу.
// Ушёл в небо — свет неба: сверху тот же цвет, что у «света неба» быстрого режима, снизу —
// GROUND; луч глаза и зеркальные отскоки видят само небо (sky), как в быстром режиме. Единицы те же, что у быстрого режима (shade_surface): цвет = альбедо × освещённость,
// поэтому картинки можно сравнивать.
//
// Сумма путей — в буфере по адресу pc.edges (4 числа на пиксель), число уже накопленных
// кадров — pc.meshes (0 — начать заново), отскоков — pc.flags. Среднее пишется в картинку
// pc.target, её растягивает «вывод».
//
// Режим «качество» (путь ReSTIR, шаг 2, бит PATH_LIVE в pc.flags): путь начинается с
// точки буфера геометрии (лучи глаза и края — как у быстрого режима), по одному пути на
// пиксель без накопления; pc.meshes — номер кадра (для случайных чисел). Сумма не пишется.
//
// Не видно: модели из треугольников (их нет в поле сцены).

[[vk::binding(2, 0)]] [format("rgba16f")] RWTexture2D<float4> images[];

#include "include/march.slang"
#include "include/shade3d.slang"
#include "include/shadow3d.slang"
#include "include/trace3d.slang"

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    return float4(0.0, 0.0, 0.0, 0.0);
}

static const float PI = 3.14159265;
// Угловой радиус диска солнца, рад: полутень того же размера, что у мягких теней быстрого
// режима (k = 12 в shadow_part).
static const float SUN_RADIUS = 0.04;
// Радиус светящегося шарика лампы для полутени, м.
static const float LAMP_RADIUS = 0.1;
static const float3 SKY_TOP = float3(0.25, 0.30, 0.40);
static const float3 SUN_COLOR = float3(1.0, 0.95, 0.85);

static const uint PATH_LIVE = 16;

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

// Равномерно в конусе вокруг `axis` с половинным углом `angle`.
float3 cone_dir(float3 axis, float angle)
{
    float3 t, b;
    basis(axis, t, b);
    float c = 1.0 - rnd() * (1.0 - cos(angle));
    float s = sqrt(max(1.0 - c * c, 0.0));
    float phi = 2.0 * PI * rnd();
    return normalize(t * (s * cos(phi)) + b * (s * sin(phi)) + axis * c);
}

// Свет неба для луча, ушедшего мимо всего.
float3 sky_light(float3 dir)
{
    return dir.y >= 0.0 ? SKY_TOP : GROUND;
}

// Прямой свет солнца и ламп в точке `q` (уже поднятой над поверхностью) с нормалью `n`;
// луч пришёл по `dir`. Как в shade_surface: альбедо × освещённость плюс блик.
float3 direct_light(float3 q, float3 n, float3 dir, SurfaceMaterial sm)
{
    float3 color = float3(0.0, 0.0, 0.0);
    float shine = 0.0;
    if ((pc.scene[11] & SCENE_NO_SUN) == 0)
    {
        float3 l = cone_dir(SUN, SUN_RADIUS);
        float c = dot(n, l);
        if (c > 0.0 && !occluded(q, l, 30.0))
        {
            color += sm.albedo * SUN_COLOR * c;
            shine += pow(saturate(dot(n, normalize(SUN - dir))), 48.0);
        }
    }
    uint lamps = lamp_count();
    for (uint i = 0; i < lamps; i++)
    {
        Lamp lamp = read_lamp(i);
        float3 to = lamp.at - q;
        float distance = length(to);
        float3 ldir = to / max(distance, 1e-4);
        float amount = lamp_falloff(lamp, distance) * saturate(dot(n, ldir));
        if (amount <= 0.0)
            continue;
        // Луч тени к случайной точке шарика лампы; останавливается, не дойдя до неё
        // LAMP_SHADOW_GAP, как в shadow_part (лампочка не закрывает свою лампу).
        float3 aim = lamp.at + (cone_dir(-ldir, PI * 0.5) * LAMP_RADIUS) - q;
        float far = length(aim) - LAMP_SHADOW_GAP;
        if (far > 0.0 && occluded(q, aim / max(length(aim), 1e-4), far))
            continue;
        color += sm.albedo * lamp.color * amount;
        shine += amount * pow(saturate(dot(n, normalize(ldir - dir))), 48.0);
    }
    if (!sm.grid_on)
        color += 0.25 * shine;
    return color;
}

// Свет, пришедший в глаз по лучу из `origin` по `dir` (длина до дальней плоскости
// `limit`). `spread` — ширина пикселя на метр пути, `px` — пиксель.
// `first` — готовое первое попадание (x — длина, −1 — небо; y — материал; zw — нормаль
// в октаэдре), если `known`.
float3 path(float3 origin, float3 dir, float limit, float spread, int2 px, uint bounces, bool known, float4 first)
{
    float3 light = float3(0.0, 0.0, 0.0);
    float3 weight = float3(1.0, 1.0, 1.0);
    float travelled = 0.0;
    float first_t = -1.0;
    float3 first_dir = dir;
    // Путь ещё не рассеивался (глаз или зеркальные отскоки): небо видно как небо.
    bool seen = true;
    for (uint k = 0; k <= bounces; k++)
    {
        bool plane;
        float3 n;
        float2 hit;
        if (k == 0 && known)
        {
            hit = first.xy;
            plane = true;
            n = oct_decode(first.zw);
        }
        else
            hit = trace_eye(origin, dir, k == 0 ? limit : 80.0, plane, n);
        if (hit.x < 0.0)
        {
            light += weight * (seen ? sky(dir) : sky_light(dir));
            break;
        }
        float3 p = origin + dir * hit.x;
        travelled += hit.x;
        if (k == 0)
            first_t = hit.x;
        if (!plane)
        {
            float mean;
            n = normal_at(p, travelled, false, mean);
        }
        float3 side, up;
        basis(dir, side, up);
        float w = spread * travelled;
        SurfaceMaterial sm = surface_material(p, dir, hit.y, n, side * w, up * w, px, -1, false);
        // Шарик лампы, встреченный после рассеяния, не светит: свет лампы уже взят явным
        // лучом к ней (иначе он учёлся бы дважды, и были бы яркие точки).
        if (seen || (pc.scene[sm.at + 3] & MATERIAL_BULB) == 0)
            light += weight * sm.albedo * sm.glow;
        float3 q = p + n * (0.01 + 0.002 * travelled);
        float shine = asfloat(pc.scene[sm.at + 6]);
        float share = shine > 0.0 ? reflect_share(shine, n, dir) : 0.0;
        // Прямой свет — всегда, с долей рассеянного: случайным выбирается только, куда
        // идёт путь дальше (иначе у зеркал редкие яркие точки рассеянного света).
        if (share < 1.0)
            light += weight * (1.0 - share) * direct_light(q, n, dir, sm);
        if (rnd() < share)
        {
            // Зеркальный отскок: с вероятностью доли Френеля, поэтому вес тот же.
            float3 r = reflect(dir, n);
            float rough = asfloat(pc.scene[sm.at + 7]);
            if (rough > 0.0)
            {
                float3 g = cone_dir(r, rough * 0.5);
                r = dot(g, n) > 0.0 ? g : r;
            }
            origin = q;
            dir = r;
            continue;
        }
        // Рассеянный отскок: с вероятностью 1 − доля, вклад (1 − доля) × цвет.
        weight *= sm.albedo;
        seen = false;
        // Русская рулетка: тусклые пути обрываются, оставшиеся весят больше.
        if (k >= 2)
        {
            float keep = clamp(max(weight.x, max(weight.y, weight.z)), 0.05, 0.95);
            if (rnd() > keep)
                break;
            weight /= keep;
        }
        origin = q;
        dir = cosine_dir(n);
    }
    // Дымка — как у быстрого режима, по первой точке.
    if (first_t >= 0.0)
    {
        float fog = 1.0 - exp(-0.0025 * first_t * first_t * 0.1);
        light = lerp(light, sky(first_dir), saturate(fog));
    }
    return light;
}

[shader("compute")]
[numthreads(8, 8, 1)]
void cs_main(uint3 id : SV_DispatchThreadID)
{
    uint w, h;
    images[pc.target].GetDimensions(w, h);
    if (id.x >= w || id.y >= h)
        return;
    int2 px = int2(id.xy);
    uint frame = pc.meshes;
    g_rng = hash(id.x * 1973u + id.y * 9277u + hash(frame) * 26699u) | 1u;
    bool live = (pc.flags & PATH_LIVE) != 0;
    float2 jitter = live ? float2(0.5, 0.5) : float2(rnd(), rnd());
    float2 screen = float2((float(px.x) + jitter.x) / float(w) * 2.0 - 1.0, 1.0 - (float(px.y) + jitter.y) / float(h) * 2.0);
    Ray ray = screen_ray(screen);
    Ray next = screen_ray(screen + float2(2.0 / float(w), 0.0));
    float spread = length(next.dir - ray.dir);
    float4 first = live ? gbuffer_load(px) : float4(0.0, 0.0, 0.0, 0.0);
    float3 c = path(ray.origin, ray.dir, ray.limit, spread, px, pc.flags & 15, live, first);
    // Светлячки (редкий путь в яркое) не дают NaN и бесконечностей.
    if (any(isnan(c)) || any(isinf(c)))
        c = float3(0.0, 0.0, 0.0);
    if (live)
    {
        images[pc.target][id.xy] = float4(c, 1.0);
        return;
    }
    uint at = (id.y * w + id.x) * 4;
    float3 sum = c;
    if (frame > 0)
        sum += float3(asfloat(pc.edges[at]), asfloat(pc.edges[at + 1]), asfloat(pc.edges[at + 2]));
    pc.edges[at] = asuint(sum.x);
    pc.edges[at + 1] = asuint(sum.y);
    pc.edges[at + 2] = asuint(sum.z);
    images[pc.target][id.xy] = float4(sum / float(frame + 1), 1.0);
}

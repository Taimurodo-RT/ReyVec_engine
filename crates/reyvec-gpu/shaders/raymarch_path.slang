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
// GROUND; луч глаза и зеркальные отскоки видят само небо (sky), как в быстром режиме.
// Единицы те же, что у быстрого режима (shade_surface): цвет = альбедо × освещённость,
// поэтому картинки можно сравнивать. Точки света и случайные числа — include/light3d.slang.
//
// Сумма путей — в буфере по адресу pc.edges (4 числа на пиксель), число уже накопленных
// кадров — pc.meshes (0 — начать заново), отскоков — pc.flags. Среднее пишется в картинку
// pc.target, её растягивает «вывод».
//
// Режим «качество» (путь ReSTIR, шаг 2, бит PATH_LIVE в pc.flags): путь начинается с
// точки буфера геометрии (лучи глаза и края — как у быстрого режима), по одному пути на
// пиксель без накопления; pc.meshes — номер кадра (для случайных чисел). Сумма не пишется.
// С битом PATH_RESTIR (шаг 3) прямой свет первой точки — ReSTIR DI: pc.edges — буфер
// запасов (include/restir.slang).
// С битом PATH_GI (шаг 4) отражённый свет первой точки — ReSTIR GI (include/restir_gi.slang):
// путь ведёт от неё только отражённую долю (всегда, с весом доли), а рассеянное
// продолжение — отдельный путь (gi_path), который не входит в картинку, а становится
// кандидатом запаса (слой 1, вместе с запасом этого места из прошлого кадра); свет из
// запасов соседей добавит проход «ReSTIR GI: соседи». Нужен PATH_SPLIT. Зеркала — как без GI.
//
// Не видно: модели из треугольников (их нет в поле сцены).

[[vk::binding(2, 0)]] [format("rgba16f")] RWTexture2D<float4> images[];

#include "include/march.slang"
#include "include/shade3d.slang"
#include "include/shadow3d.slang"
#include "include/trace3d.slang"
#include "include/light3d.slang"
#include "include/restir.slang"
#include "include/restir_gi.slang"

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    return float4(0.0, 0.0, 0.0, 0.0);
}

static const uint PATH_LIVE = 16;
// Бит pc.flags (шаг 3): прямой свет первой точки — ReSTIR DI (include/restir.slang); запасы
// этого кадра до соседей готовит проход «ReSTIR: кандидаты».
static const uint PATH_RESTIR = 32;
// Бит pc.flags (шаг 5): свет для шумодава — в pc.target свет без цвета первой точки, в
// pc.cone (номер в images[]) — цвет первой точки и доля дымки.
static const uint PATH_SPLIT = 64;
// Бит pc.flags (шаг 4): отражённый свет первой точки — ReSTIR GI.
static const uint PATH_GI = 128;
// Сколько соседей берёт пиксель и как далеко (пикселей).
static const uint RESTIR_NEIGHBORS = 4;
static const float RESTIR_RADIUS = 16.0;
static const float RESTIR_REACH = 0.5;

// Прямой свет солнца и ламп в точке `q` (уже поднятой над поверхностью) с нормалью `n`;
// луч пришёл по `dir`. По случайной точке каждого источника с лучом тени.
float3 direct_light(float3 q, float3 n, float3 dir, SurfaceMaterial sm)
{
    float3 color = float3(0.0, 0.0, 0.0);
    color += sun_light(q, n, dir, sm);
    uint lamps = lamp_count();
    for (uint i = 0; i < lamps; i++)
        color += light_seen(light_sample(i + 1u, float2(rnd(), rnd()), q, n, dir, sm), q);
    return color;
}

// Прямой свет дальних точек пути в режиме ReSTIR: несколько случайных точек света, одна
// по весам — и один луч тени к ней (как запас ReSTIR, но без соседей и прошлого кадра):
// ламп может быть много, а луч тени один.
static const uint RIS_CANDIDATES = 16;

float3 sampled_light(float3 q, float3 n, float3 dir, SurfaceMaterial sm)
{
    float3 sun = sun_light(q, n, dir, sm);
    uint total = lamp_count();
    if (total == 0u)
        return sun;
    ReservoirBuilder b = builder_new(q, n);
    for (uint k = 0; k < RIS_CANDIDATES; k++)
    {
        uint light = pick_lamp(rnd());
        float2 uv = float2(rnd(), rnd());
        float target = luminance(light_sample(light, uv, q, n, dir, sm).light);
        builder_add(b, light, uv, target, target * float(total), 1.0);
    }
    Reservoir r = builder_finish(b);
    if (r.light == RESTIR_EMPTY)
        return sun;
    return sun + light_seen(light_sample(r.light, r.uv, q, n, dir, sm), q) * r.W;
}

// Прямой свет первой точки через ReSTIR DI: свой запас и запасы соседей (слой 1), один
// луч тени к выбранной точке лампы и один — к солнцу. Итог (с тенью: закрытая точка теряет вес) — в слой 0, его
// возьмёт следующий кадр.
float3 restir_direct(int2 px, uint w, uint h, float3 p, float3 n, float3 q, float3 dir,
                     SurfaceMaterial sm, float t, float spread)
{
    ReservoirBuilder b = builder_new(p, n);
    builder_merge(b, reservoir_load(1u, px, w, h), 1e30, q, dir, sm, t);
    // Соседи — не дальше RESTIR_REACH метров по поверхности: вдали 16 пикселей — это
    // метры, и свет там уже другой.
    float radius = clamp(RESTIR_REACH / max(spread * t, 1e-5), 2.0, RESTIR_RADIUS);
    for (uint k = 0; k < RESTIR_NEIGHBORS; k++)
    {
        float r = radius * sqrt(rnd());
        float phi = 2.0 * PI * rnd();
        int2 o = px + int2(round(float2(cos(phi), sin(phi)) * r));
        if (all(o == px) || any(o < 0) || o.x >= int(w) || o.y >= int(h))
            continue;
        builder_merge(b, reservoir_load(1u, o, w, h), 1e30, q, dir, sm, t);
    }
    Reservoir chosen = builder_finish(b);
    float3 direct = sun_light(q, n, dir, sm);
    if (chosen.light != RESTIR_EMPTY)
    {
        float3 seen = light_seen(light_sample(chosen.light, chosen.uv, q, n, dir, sm), q);
        if (all(seen <= 0.0))
            chosen.W = 0.0;
        direct += seen * chosen.W;
    }
    chosen.M = min(chosen.M, RESTIR_HISTORY * float(RESTIR_CANDIDATES));
    reservoir_store(0u, px, w, h, chosen);
    return direct;
}

// Свет, пришедший в глаз по лучу из `origin` по `dir` (длина до дальней плоскости
// `limit`). `spread` — ширина пикселя на метр пути, `px` — пиксель.
// `first` — готовое первое попадание (x — длина, −1 — небо; y — материал; zw — нормаль
// в октаэдре), если `known`. Дымку путь не добавляет: `first_dist` — длина до первой точки
// (−1 — небо), `first_albedo` — её цвет (для шумодава, который чистит свет без цвета),
// `first_normal` — нормаль в ней. `seen` — путь ещё не рассеивался (небо видно как небо).
// С `gi` рассеянное продолжение первой точки путь не ведёт (его свет — ReSTIR GI): там, где
// оно было бы, `gi_found`, а `gi_q`/`gi_n` — точка над поверхностью и нормаль, откуда его
// вести (gi_path). Отражённая доля тогда идёт всегда, с весом доли.
float3 path(float3 origin, float3 dir, float limit, float spread, int2 px, uint bounces, bool known, float4 first,
           bool restir, bool gi, bool seen, uint w, uint h, out float first_dist, out float3 first_albedo,
           out float3 first_normal, out bool gi_found, out float3 gi_q, out float3 gi_n)
{
    first_albedo = float3(1.0, 1.0, 1.0);
    first_normal = -dir;
    gi_found = false;
    gi_q = float3(0.0, 0.0, 0.0);
    gi_n = float3(0.0, 1.0, 0.0);
    float3 light = float3(0.0, 0.0, 0.0);
    float3 weight = float3(1.0, 1.0, 1.0);
    float travelled = 0.0;
    float first_t = -1.0;
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
        if (k == 0)
            first_normal = n;
        float3 side, up;
        basis(dir, side, up);
        float width = spread * travelled;
        SurfaceMaterial sm = surface_material(p, dir, hit.y, n, side * width, up * width, px, -1, false);
        if (k == 0)
            first_albedo = max(sm.albedo, float3(0.03, 0.03, 0.03));
        // Шарик лампы, встреченный после рассеяния, не светит: свет лампы уже взят явным
        // лучом к ней (иначе он учёлся бы дважды, и были бы яркие точки).
        if (seen || (pc.scene[sm.at + 3] & MATERIAL_BULB) == 0)
            light += weight * sm.albedo * sm.glow;
        float3 q = p + n * (0.01 + 0.002 * travelled);
        float shine = asfloat(pc.scene[sm.at + 6]);
        float share = shine > 0.0 ? reflect_share(shine, n, dir) : 0.0;
        // Прямой свет — всегда, с долей рассеянного: случайным выбирается только, куда
        // идёт путь дальше (иначе у зеркал редкие яркие точки рассеянного света).
        float3 direct = float3(0.0, 0.0, 0.0);
        if (k == 0 && restir && known)
            direct = restir_direct(px, w, h, p, n, q, dir, sm, hit.x, spread);
        else if (share < 1.0)
            direct = restir ? sampled_light(q, n, dir, sm) : direct_light(q, n, dir, sm);
        light += weight * (1.0 - share) * direct;
        // ReSTIR GI: рассеянное продолжение — отдельно (gi_path), отражённое — всегда.
        bool split = k == 0 && gi && bounces > 0 && !gi_mirror(gi_material(hit.y));
        if (split)
        {
            gi_found = true;
            gi_q = q;
            gi_n = n;
            if (share <= 0.0)
                break;
            weight *= share;
        }
        if (split || rnd() < share)
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
    first_dist = first_t;
    return light;
}

// Рассеянное продолжение первой точки для ReSTIR GI: из точки `q` с нормалью `n` — отскок
// по косинусу и путь дальше (на отскок короче). Точка x_s, куда он пришёл (`pos`, `normal`;
// небо — далеко по лучу), и её свет — кандидат запаса. `restir` — прямой свет по выбору
// (RIS), как у дальних точек основного пути.
float3 gi_path(float3 q, float3 n, float spread, int2 px, uint bounces, bool restir, uint w, uint h, out float3 pos,
               out float3 normal)
{
    float3 dir = cosine_dir(n);
    float d;
    float3 albedo, inner_q, inner_n;
    bool inner;
    float3 radiance = path(q, dir, 80.0, spread, px, bounces - 1u, false, float4(0.0, 0.0, 0.0, 0.0), restir, false,
                           false, w, h, d, albedo, normal, inner, inner_q, inner_n);
    pos = d >= 0.0 ? q + dir * d : q + dir * GI_SKY;
    if (d < 0.0)
        normal = -dir;
    if (any(isnan(radiance)) || any(isinf(radiance)))
        radiance = float3(0.0, 0.0, 0.0);
    return max(radiance, float3(0.0, 0.0, 0.0));
}

// Кандидат ReSTIR GI пикселя `px` (первая точка — `first`) и запас этого места в прошлом
// кадре — в слой 1.
void gi_candidate(int2 px, uint w, uint h, Ray ray, float4 first, bool found, float3 pos, float3 normal,
                  float3 radiance)
{
    if (first.x < 0.0 || gi_mirror(gi_material(first.y)))
    {
        gi_store(1u, px, w, h, gi_empty(float3(0.0, 0.0, 0.0), float3(0.0, 1.0, 0.0)));
        return;
    }
    float3 p = ray.origin + ray.dir * first.x;
    float3 n = oct_decode(first.zw);
    float3 q = p + n * (0.01 + 0.002 * first.x);
    GiBuilder b = gi_builder(q, n);
    if (found)
        gi_add_own(b, pos, normal, radiance, q);
    int2 prev;
    if (previous_pixel(p, w, h, PREV_GI, prev))
        gi_merge(b, gi_load(0u, prev, w, h), GI_HISTORY, q, first.x);
    gi_store(1u, px, w, h, gi_finish(b));
}

// Дымка — как у быстрого режима, по первой точке (длина `t`, −1 — небо).
float fog_at(float t)
{
    return t >= 0.0 ? saturate(1.0 - exp(-0.0025 * t * t * 0.1)) : 0.0;
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
    seed_rng(id.xy, frame, 0u);
    bool live = (pc.flags & PATH_LIVE) != 0;
    bool restir = live && (pc.flags & PATH_RESTIR) != 0;
    float2 jitter = live ? float2(0.5, 0.5) : float2(rnd(), rnd());
    float2 screen = float2((float(px.x) + jitter.x) / float(w) * 2.0 - 1.0, 1.0 - (float(px.y) + jitter.y) / float(h) * 2.0);
    Ray ray = screen_ray(screen);
    Ray next = screen_ray(screen + float2(2.0 / float(w), 0.0));
    float spread = length(next.dir - ray.dir);
    float4 first = live ? gbuffer_load(px) : float4(0.0, 0.0, 0.0, 0.0);
    bool gi = live && (pc.flags & PATH_GI) != 0;
    float first_dist;
    float3 albedo;
    float3 first_normal;
    bool gi_found;
    float3 gi_q, gi_n;
    uint bounces = pc.flags & 15;
    float3 c = path(ray.origin, ray.dir, ray.limit, spread, px, bounces, live, first, restir, gi, true, w, h,
                    first_dist, albedo, first_normal, gi_found, gi_q, gi_n);
    if (gi)
    {
        float3 gi_pos = float3(0.0, 0.0, 0.0), gi_normal = float3(0.0, 1.0, 0.0);
        float3 gi_radiance = float3(0.0, 0.0, 0.0);
        if (gi_found)
            gi_radiance = gi_path(gi_q, gi_n, spread, px, bounces, restir, w, h, gi_pos, gi_normal);
        gi_candidate(px, w, h, ray, first, gi_found, gi_pos, gi_normal, gi_radiance);
    }
    // Светлячки (редкий путь в яркое) не дают NaN и бесконечностей.
    if (any(isnan(c)) || any(isinf(c)))
        c = float3(0.0, 0.0, 0.0);
    float fog = fog_at(first_dist);
    if ((pc.flags & PATH_SPLIT) != 0)
    {
        // Для шумодава: свет без цвета первой точки и без дымки, цвет и дымка — отдельно
        // (их соберёт «шумодав: сборка»).
        images[pc.target][id.xy] = float4(c / albedo, 1.0);
        images[pc.cone][id.xy] = float4(albedo, fog);
        return;
    }
    c = lerp(c, sky(ray.dir), fog);
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

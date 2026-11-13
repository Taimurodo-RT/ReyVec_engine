// Освещение точки сцены «3D» (этапы 7.3–7.4): общее для прохода «сцена»
// (raymarch_shade.slang) и краёв форм (7.6о, raymarch_edges.slang). Перед подключением
// должен быть подключён space.slang (с ним приходит и список картинок `textures[]`).
// Буфер геометрии — `gbuffer_load` (pc.gbuffer, space.slang), тени — `textures[pc.shadows]`.

// Небо: к горизонту светлее.
float3 sky(float3 dir)
{
    float t = saturate(dir.y * 0.5 + 0.5);
    return lerp(float3(0.30, 0.32, 0.36), float3(0.05, 0.09, 0.18), t);
}

// Линии сетки на полу: доля пикселя, закрытая линией шириной `width` метров через
// каждые `step` метров; `fw` — сколько метров в пикселе.
float grid(float2 p, float step, float width, float2 fw)
{
    float2 d = abs(frac(p / step + 0.5) - 0.5) * step;
    float2 cover = saturate(1.0 - (d - width * 0.5) / max(fw, 1e-5));
    return max(cover.x, cover.y);
}

static const uint MATERIAL_GRID = 1;
// Свет снизу: отражённый от освещённого пола (7.11). Без него низ форм, смотрящий в пол,
// не получал света вовсе и у пола выходил чёрной полосой.
static const float3 GROUND = float3(0.12, 0.12, 0.13);

// Тени для пикселя `px` из половинного разрешения (7.6б). Точка теней h посчитана для
// пикселя кадра 2h, поэтому пиксель кадра лежит между четырьмя ближайшими точками.
// Каждая берётся с весом по расстоянию (как при обычном растяжении) и по сходству с
// пикселем: та же глубина и та же сторона. Так тень не перетекает с переднего
// предмета на пол за ним. Если похожих нет среди четырёх, ищем в квадрате 4×4.
// `similar` — сколько веса набрали четыре ближайшие точки (0..1): меньше половины —
// тени пикселя растянуты из чужой или далёкой поверхности, ненадёжны (7.6с).
float4 upsampled_shadows_checked(int2 px, float t, float3 n, out float similar)
{
    uint sw, sh;
    int2 gsize = gbuffer_size();
    textures[pc.shadows].GetDimensions(sw, sh);
    int2 base = px >> 1;
    float2 f = float2(px & 1) * 0.5;
    float4 sum = float4(0.0, 0.0, 0.0, 0.0);
    float total = 0.0;
    float4 nearest = textures[pc.shadows].Load(int3(min(base, int2(sw, sh) - 1), 0));
    for (uint k = 0; k < 4; k++)
    {
        int2 o = int2(k & 1, k >> 1);
        int2 hp = min(base + o, int2(sw, sh) - 1);
        float bilinear = (o.x == 1 ? f.x : 1.0 - f.x) * (o.y == 1 ? f.y : 1.0 - f.y);
        if (bilinear <= 0.0)
            continue;
        float4 g = gbuffer_load(min(hp * 2, gsize - 1));
        if (g.x < 0.0)
            continue;
        float depth = saturate(1.0 - abs(g.x - t) / (0.02 * t + 0.05));
        float facing = saturate(dot(oct_decode(g.zw), n));
        facing *= facing;
        facing *= facing;
        float weight = bilinear * depth * facing;
        sum += textures[pc.shadows].Load(int3(hp, 0)) * weight;
        total += weight;
    }
    similar = total;
    if (total > 1e-4)
        return sum / total;
    // Ни одна из четырёх точек не похожа на пиксель: тонкая перемычка или край формы,
    // где вокруг другая поверхность. Раньше бралась ближняя точка как есть — и пиксель
    // получал чужую тень (тёмные точки на краях). Теперь ищем похожие точки шире,
    // квадратом 4×4 вокруг, уже без веса по расстоянию.
    for (uint k = 0; k < 16; k++)
    {
        int2 hp = clamp(base + int2(int(k & 3) - 1, int(k >> 2) - 1), int2(0, 0), int2(sw, sh) - 1);
        float4 g = gbuffer_load(min(hp * 2, gsize - 1));
        if (g.x < 0.0)
            continue;
        float depth = saturate(1.0 - abs(g.x - t) / (0.02 * t + 0.05));
        float facing = saturate(dot(oct_decode(g.zw), n));
        facing *= facing;
        float weight = depth * facing * facing;
        sum += textures[pc.shadows].Load(int3(hp, 0)) * weight;
        total += weight;
    }
    return total > 1e-4 ? sum / total : nearest;
}

// Тени пикселя без проверки (см. upsampled_shadows_checked).
float4 upsampled_shadows(int2 px, float t, float3 n)
{
    float similar;
    return upsampled_shadows_checked(px, t, n, similar);
}

// То же с готовыми долями света `shadows` (солнце, углы, две лампы).
float3 shade_lit(float2 screen, float4 g, float4 shadows)
{
    Ray ray = screen_ray(screen);
    float t = g.x;
    if (t < 0.0)
        return sky(ray.dir);
    // Как меняется луч к соседнему пикселю: лучи через соседние точки экрана. Не ddx:
    // так же считается и в вычислительном шейдере краёв, где производных нет.
    float2 size = float2(round(pc.aspect * pc.height), pc.height);
    Ray rx = screen_ray(screen + float2(2.0 / size.x, 0.0));
    Ray ry = screen_ray(screen - float2(0.0, 2.0 / size.y));
    float3 dox = rx.origin - ray.origin, doy = ry.origin - ray.origin;
    float3 ddx_dir = rx.dir - ray.dir, ddy_dir = ry.dir - ray.dir;
    float3 p = ray.origin + ray.dir * t;

    uint material = min(uint(g.y + 0.5), max(pc.scene[3], 1) - 1);
    uint m = pc.scene[2] + material * 8;
    float3 albedo = float3(asfloat(pc.scene[m]), asfloat(pc.scene[m + 1]), asfloat(pc.scene[m + 2]));
    bool grid_on = (pc.scene[m + 3] & MATERIAL_GRID) != 0;
    float glow = asfloat(pc.scene[m + 4]);
    float3 n = oct_decode(g.zw);
    if (grid_on)
    {
        // Сколько метров пола в пикселе: соседний луч пересекается с той же плоскостью.
        float facing = min(dot(n, ray.dir), -1e-4);
        float3 ax = dox + ddx_dir * t;
        float3 ay = doy + ddy_dir * t;
        ax -= ray.dir * (dot(n, ax) / facing);
        ay -= ray.dir * (dot(n, ay) / facing);
        float2 fw = abs(ax.xz) + abs(ay.xz);
        // Сетка: тонкие линии каждый метр, яркие каждые 10 м; оси X и Z цветом.
        float minor = grid(p.xz, 1.0, 0.02, fw);
        float major = grid(p.xz, 10.0, 0.05, fw);
        albedo = lerp(albedo, float3(0.34, 0.36, 0.40), minor * 0.6);
        albedo = lerp(albedo, float3(0.55, 0.58, 0.62), major);
        float2 axis = saturate(1.0 - (abs(p.zx) - 0.025) / max(fw.yx, 1e-5));
        albedo = lerp(albedo, float3(0.85, 0.15, 0.12), axis.x); // ось X (вдоль z = 0)
        albedo = lerp(albedo, float3(0.15, 0.35, 0.95), axis.y); // ось Z (вдоль x = 0)
    }

    // Солнце с мягкой тенью; свет неба сверху и отражённый от пола снизу, их затеняют углы.
    float sun_light = saturate(dot(n, SUN)) * shadows.x;
    float3 light = float3(1.0, 0.95, 0.85) * sun_light;
    float3 ambient = float3(0.25, 0.30, 0.40) * (0.5 + 0.5 * n.y) + GROUND * (0.5 - 0.5 * n.y);
    light += ambient * shadows.y;
    // Лампы: у двух первых тени из прохода «тени 3D», остальные светят без теней.
    float3 h = normalize(SUN - ray.dir);
    float specular = pow(saturate(dot(n, h)), 48.0) * shadows.x;
    uint lamps = lamp_count();
    for (uint i = 0; i < lamps; i++)
    {
        Lamp l = read_lamp(i);
        float3 to = l.at - p;
        float distance = length(to);
        float3 dir = to / max(distance, 1e-4);
        float shadow = i == 0 ? shadows.z : (i == 1 ? shadows.w : 1.0);
        float amount = lamp_falloff(l, distance) * saturate(dot(n, dir)) * shadow;
        light += l.color * amount;
        specular += amount * pow(saturate(dot(n, normalize(dir - ray.dir))), 48.0);
    }
    float3 color = albedo * light;
    // Блик у форм (не у пола с сеткой).
    if (!grid_on)
        color += 0.25 * specular;
    // Свечение: своя яркость, тень и свет на неё не влияют. Ярче 1 — ореол при выводе.
    color += albedo * glow;
    // Даль уходит в дымку неба (сетка не рябит у горизонта).
    float fog = 1.0 - exp(-0.0025 * t * t * 0.1);
    return lerp(color, sky(ray.dir), saturate(fog));
}

// Цвет точки буфера геометрии `g`, полученной лучом через точку экрана `screen`
// пикселя `px` (тени берутся вокруг этого пикселя).
float3 shade_point(float2 screen, float4 g, int2 px)
{
    if (g.x < 0.0)
        return sky(screen_ray(screen).dir);
    return shade_lit(screen, g, upsampled_shadows(px, g.x, oct_decode(g.zw)));
}


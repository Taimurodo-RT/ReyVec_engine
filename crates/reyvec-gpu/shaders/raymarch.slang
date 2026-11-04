// Сцена «3D» (этап 7.1): первый объёмный вид. Каждая точка экрана пускает луч от камеры
// и шагает по нему на расстояние до ближайшей поверхности (sphere tracing), пока не
// упрётся в неё. Формы пока заданы прямо здесь: пол с сеткой (клетка 1 м, каждая десятая
// линия ярче), оси X (красная) и Z (синяя), шар, куб и тор — чтобы было видно, что
// перспектива и полёт камерой работают. Формы из данных (байт-код) — задача 7.2, буфер
// глубины и нормалей — 7.3, мягкие тени — 7.4.
//
// Луч строится так же, как Camera3D::ray в crates/reyvec-sdf/src/camera3d.rs: точка
// экрана через обратную матрицу камеры ложится на ближнюю (глубина 1) и дальнюю
// (глубина 0) плоскости. Матрица приходит столбцами.

struct Push
{
    float4 inv0; // столбцы обратной матрицы «мир → экран»
    float4 inv1;
    float4 inv2;
    float4 inv3;
    float3 eye;  // где глаз (для плоской камеры не нужен: начало луча из матрицы)
    float time;  // секунды с запуска
    float aspect; // ширина / высота
    float height; // высота кадра в пикселях
    uint flags;
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

// Формы сцены. Номер материала: 0 — пол, 1 — шар, 2 — куб, 3 — тор.
float sd_sphere(float3 p, float r)
{
    return length(p) - r;
}

float sd_box(float3 p, float3 half_size, float round)
{
    float3 q = abs(p) - half_size + round;
    return length(max(q, 0.0)) + min(max(q.x, max(q.y, q.z)), 0.0) - round;
}

float sd_torus(float3 p, float big, float small)
{
    float2 q = float2(length(p.xz) - big, p.y);
    return length(q) - small;
}

// Расстояние до ближайшей формы (x) и её материал (y).
float2 scene(float3 p)
{
    float2 best = float2(p.y, 0.0);
    float bob = 0.15 * sin(pc.time * 1.3);
    float d = sd_sphere(p - float3(0.0, 1.0 + bob, 0.0), 1.0);
    if (d < best.x)
        best = float2(d, 1.0);
    d = sd_box(p - float3(3.0, 0.75, -0.5), float3(0.75, 0.75, 0.75), 0.08);
    if (d < best.x)
        best = float2(d, 2.0);
    // Тор стоит на ребре и медленно крутится вокруг вертикали.
    float a = pc.time * 0.5;
    float3 q = p - float3(-3.2, 1.0, 0.3);
    q = float3(q.x * cos(a) - q.z * sin(a), q.y, q.x * sin(a) + q.z * cos(a));
    d = sd_torus(float3(q.x, q.z, q.y), 0.75, 0.25);
    if (d < best.x)
        best = float2(d, 3.0);
    return best;
}

float3 normal_at(float3 p)
{
    // Наклон поля по четырём точкам тетраэдра: дешевле шести и без перекоса.
    const float e = 0.001;
    const float2 k = float2(1.0, -1.0);
    return normalize(
        k.xyy * scene(p + k.xyy * e).x + k.yyx * scene(p + k.yyx * e).x +
        k.yxy * scene(p + k.yxy * e).x + k.xxx * scene(p + k.xxx * e).x);
}

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

static const float3 MATERIALS[4] = {
    float3(0.18, 0.19, 0.21), // пол
    float3(0.85, 0.35, 0.20), // шар
    float3(0.25, 0.55, 0.85), // куб
    float3(0.90, 0.75, 0.25), // тор
};

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    float3 origin = unproject(input.screen, 1.0);
    float3 far = unproject(input.screen, 0.0);
    float3 dir = normalize(far - origin);
    float limit = length(far - origin);

    // Шагаем по лучу: каждый шаг — на расстояние до ближайшей формы, ближе неё ничего нет.
    float t = 0.0;
    float2 hit = float2(1e9, -1.0);
    for (int i = 0; i < 160; i++)
    {
        float2 s = scene(origin + dir * t);
        // Точность растёт к камере: сотая доля миллиметра на метр пути.
        if (s.x < 0.0005 * max(t, 1.0))
        {
            hit = float2(t, s.y);
            break;
        }
        t += s.x;
        if (t > limit)
            break;
    }
    if (hit.y < 0.0)
        return float4(sky(dir), 1.0);

    float3 p = origin + dir * hit.x;
    uint material = uint(hit.y);
    float3 n = material == 0 ? float3(0.0, 1.0, 0.0) : normal_at(p);
    float3 albedo = MATERIALS[material];
    if (material == 0)
    {
        // Сетка: тонкие линии каждый метр, яркие каждые 10 м; оси X и Z цветом.
        float2 fw = fwidth(p.xz);
        float minor = grid(p.xz, 1.0, 0.02, fw);
        float major = grid(p.xz, 10.0, 0.05, fw);
        albedo = lerp(albedo, float3(0.34, 0.36, 0.40), minor * 0.6);
        albedo = lerp(albedo, float3(0.55, 0.58, 0.62), major);
        float2 axis = saturate(1.0 - (abs(p.zx) - 0.025) / max(fw.yx, 1e-5));
        albedo = lerp(albedo, float3(0.85, 0.15, 0.12), axis.x); // ось X (вдоль z = 0)
        albedo = lerp(albedo, float3(0.15, 0.35, 0.95), axis.y); // ось Z (вдоль x = 0)
    }

    // Простой свет: солнце сбоку сверху и рассеянный свет неба. Тени — в 7.4.
    float3 sun = normalize(float3(0.5, 0.8, 0.35));
    float diffuse = saturate(dot(n, sun));
    float3 light = float3(1.0, 0.95, 0.85) * diffuse + float3(0.25, 0.30, 0.40) * (0.5 + 0.5 * n.y);
    float3 color = albedo * light;
    // Блик у форм.
    if (material != 0)
    {
        float3 h = normalize(sun - dir);
        color += 0.25 * pow(saturate(dot(n, h)), 48.0);
    }
    // Даль уходит в дымку неба (сетка не рябит у горизонта).
    float fog = 1.0 - exp(-0.0025 * hit.x * hit.x * 0.1);
    return float4(lerp(color, sky(dir), saturate(fog)), 1.0);
}

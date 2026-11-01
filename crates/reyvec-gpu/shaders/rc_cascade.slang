// Опыт этапа 4 (задача 4.6): свет «каскадами лучей» (radiance cascades, приём Александра
// Санникова). Вместо того чтобы для каждой точки идти лучом к каждому источнику (light.slang),
// из точек-зондов во все стороны пускаются лучи, и каждый луч приносит то, что увидел:
// источник (его цвет), предмет (темноту) или ничего. Так свет сам огибает углы, проходит в
// щели, а несколько источников стоят столько же, сколько один.
//
// Хитрость в каскадах: близко к зонду картинка света меняется быстро, а направления почти
// не важны; далеко наоборот. Поэтому каскад 0 — частые зонды (в каждой точке поля) по 4
// коротких луча, каскад 1 — зонды вдвое реже по 16 лучей вчетверо длиннее, и так далее.
// Работа на каждый каскад одинаковая, а лучи вместе покрывают весь экран.
//
// Этот проход считает один каскад: каждая точка картинки — один луч одного зонда (зонд
// занимает квадрат side × side точек, side = 2^(cascade+1)). Луч идёт по полю расстояний
// своим отрезком пути; если ничего не встретил, к нему добавляется то, что видят четыре
// соседних зонда каскада выше по тем же направлениям (поэтому каскады считаются сверху
// вниз). В альфе — пропустил ли луч свет дальше (1) или упёрся (0).

struct Light
{
    float2 a;
    float2 b;
    float3 color;
    float reach;
    float softness;
    uint kind; // 0 лампа или трубка, 1 направленный
    float2 pad;
};

struct Push
{
    Light* lights;
    uint count;
    uint field;          // картинка поля расстояний (единицы сцены)
    uint upper;          // картинка каскада выше (если last == 0)
    uint cascade;
    uint last;           // 1: самый верхний каскад
    float r0;            // длина отрезка луча каскада 0, единицы сцены
    float2 center;       // камера
    float aspect;
    float zoom;
    uint2 field_size;    // размер поля в точках
    uint2 probes;        // зондов этого каскада по x и y
    uint2 upper_probes;  // зондов каскада выше
    float emit;          // яркость поверхности источника на единицу цвета
    float pad;
};
[[vk::push_constant]] ConstantBuffer<Push> pc;

[[vk::binding(0, 0)]] Texture2D textures[];
[[vk::binding(1, 0)]] SamplerState samplers[2];

struct VertexOut
{
    float4 position : SV_Position;
};

[shader("vertex")]
VertexOut vs_main(uint vertex : SV_VulkanVertexID)
{
    float2 uv = float2((vertex << 1) & 2, vertex & 2);
    VertexOut o;
    o.position = float4(uv * 2.0 - 1.0, 0.0, 1.0);
    return o;
}

static const float PI = 3.14159265;
static const int MAX_STEPS = 24;

// Точка поля (в точках поля, дробная) → точка сцены.
float2 field_to_scene(float2 q)
{
    float2 uv = q / float2(pc.field_size);
    float2 s = float2((uv.x * 2.0 - 1.0) * pc.aspect, 1.0 - uv.y * 2.0);
    return pc.center + s / pc.zoom;
}

float field_at(float2 p)
{
    float2 s = (p - pc.center) * pc.zoom;
    float2 uv = float2(s.x / pc.aspect * 0.5 + 0.5, 0.5 - s.y * 0.5);
    return textures[pc.field].SampleLevel(samplers[0], uv, 0.0).r;
}

// Ближайшие точки двух отрезков p1→q1 и p2→q2: доли s и t вдоль каждого (Эриксон,
// «Real-Time Collision Detection», 5.1.9).
float2 closest_on_segments(float2 p1, float2 q1, float2 p2, float2 q2)
{
    float2 d1 = q1 - p1;
    float2 d2 = q2 - p2;
    float2 r = p1 - p2;
    float a = dot(d1, d1);
    float e = dot(d2, d2);
    float f = dot(d2, r);
    float s;
    float t;
    if (e <= 1e-10)
    {
        t = 0.0;
        s = saturate(-dot(d1, r) / max(a, 1e-10));
    }
    else
    {
        float c = dot(d1, r);
        float b = dot(d1, d2);
        float denom = a * e - b * b;
        s = denom > 1e-10 ? saturate((b * f - c * e) / denom) : 0.0;
        t = (b * s + f) / e;
        if (t < 0.0)
        {
            t = 0.0;
            s = saturate(-c / max(a, 1e-10));
        }
        else if (t > 1.0)
        {
            t = 1.0;
            s = saturate((b - c) / max(a, 1e-10));
        }
    }
    return float2(s, t);
}

// Что видит луч из o в направлении v на отрезке пути [t0, t1].
float4 trace(float2 o, float2 v, float t0, float t1)
{
    float t = t0;
    float end = t1;
    bool blocked = false;
    // Зонд каскада 0 бывает внутри предмета: сначала луч выходит наружу (поверхность
    // предмета освещается снаружи), и только потом предметы его останавливают.
    bool outside = pc.cascade > 0;
    float step_min = 1.0 / (float(pc.field_size.y) * pc.zoom);
    for (int i = 0; i < MAX_STEPS && t < t1; i++)
    {
        float h = field_at(o + v * t);
        if (h < 0.0)
        {
            if (outside)
            {
                blocked = true;
                end = t;
                break;
            }
        }
        else
        {
            outside = true;
        }
        t += max(abs(h), step_min);
    }
    // Источники на пройденном отрезке: лампа — круг, трубка — капсула радиусом с её
    // размер (softness). Ближайший к началу луча.
    float3 seen = float3(0.0, 0.0, 0.0);
    float nearest = 1e9;
    float2 p0 = o + v * t0;
    float2 p1 = o + v * end;
    for (uint i = 0; i < pc.count; i++)
    {
        Light l = pc.lights[i];
        if (l.kind != 0)
            continue;
        float radius = max(l.softness, 0.02);
        float2 st = closest_on_segments(p0, p1, l.a, l.b);
        float2 on_ray = lerp(p0, p1, st.x);
        float2 on_light = lerp(l.a, l.b, st.y);
        if (length(on_ray - on_light) < radius && st.x < nearest)
        {
            nearest = st.x;
            // Яркость поверхности обратно пропорциональна размеру: маленькая и большая
            // лампа одного цвета светят вокруг одинаково.
            seen = l.color * pc.emit / radius;
        }
    }
    if (nearest < 1e9)
        return float4(seen, 0.0);
    if (blocked)
        return float4(0.0, 0.0, 0.0, 0.0);
    // Луч самого верхнего каскада ушёл за край: там свет издалека (луна).
    if (pc.last != 0)
    {
        float3 sky = float3(0.0, 0.0, 0.0);
        for (uint i = 0; i < pc.count; i++)
        {
            Light l = pc.lights[i];
            if (l.kind == 1)
                sky += l.color * pow(saturate(dot(v, l.a)), 6.0) * 3.0;
        }
        return float4(sky, 1.0);
    }
    return float4(0.0, 0.0, 0.0, 1.0);
}

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    uint side = 2u << pc.cascade;            // лучей зонда по стороне квадрата
    uint2 texel = uint2(input.position.xy);
    uint2 probe = texel / side;
    uint2 cell = texel % side;
    uint dir = cell.x + cell.y * side;
    uint dirs = side * side;
    float angle = (float(dir) + 0.5) * 2.0 * PI / float(dirs);
    float2 v = float2(cos(angle), sin(angle));

    // Зонды каскада i через 2^i точек поля, в середине своего квадрата.
    float spacing = float(1u << pc.cascade);
    float2 at = (float2(probe) + 0.5) * spacing;
    float2 o = field_to_scene(at);
    float scale = float(1u << (2u * pc.cascade)); // 4^i
    float t0 = pc.r0 * (scale - 1.0) / 3.0;
    float t1 = pc.r0 * (scale * 4.0 - 1.0) / 3.0;
    float4 near = trace(o, v, t0, t1);
    if (pc.last != 0 || near.a == 0.0)
        return near;

    // Дальше луч продолжают четыре соседних зонда каскада выше: их лучи в тех же
    // направлениях (4 узких вместо одного широкого), смешанные по расстоянию до зонда.
    uint side_up = side * 2u;
    float2 q = at / (spacing * 2.0) - 0.5;
    int2 base = int2(floor(q));
    float2 k = q - float2(base);
    float3 far = float3(0.0, 0.0, 0.0);
    for (int y = 0; y < 2; y++)
    {
        for (int x = 0; x < 2; x++)
        {
            int2 up = clamp(base + int2(x, y), int2(0, 0), int2(pc.upper_probes) - 1);
            float w = (x == 0 ? 1.0 - k.x : k.x) * (y == 0 ? 1.0 - k.y : k.y);
            float3 sum = float3(0.0, 0.0, 0.0);
            for (uint c = 0; c < 4; c++)
            {
                uint d = dir * 4u + c;
                uint2 at_up = uint2(up) * side_up + uint2(d % side_up, d / side_up);
                sum += textures[pc.upper].Load(int3(int2(at_up), 0)).rgb;
            }
            far += sum * 0.25 * w;
        }
    }
    return float4(near.rgb + far, 1.0);
}

// Свет сцены «лампы и тени» (задача 4.3). Считается в разрешении поля расстояний
// (jfa_field.slang): для каждой точки и каждого источника шагаем по полю в сторону
// источника. Поле говорит, на сколько можно шагнуть, не задев фигуру; если луч прошёл
// близко к фигуре, точка в полутени (чем ближе и чем дальше от точки, тем темнее). Так
// получаются мягкие тени без отдельной геометрии теней. Итог — во сколько раз осветить
// каждую точку кадра (умножает вывод, present.slang); в альфе — свечение самих ламп.

struct Light
{
    float2 a;       // лампа: точка (или начало трубки); направленный: направление на источник
    float2 b;       // конец трубки (у лампы равен a)
    float3 color;   // цвет × яркость, линейный
    float reach;    // дальность света; у направленного длина теней
    float softness; // размер источника: мягкость края тени
    uint kind;      // 0 лампа или трубка, 1 направленный
    float2 pad;
};

struct Push
{
    Light* lights;
    uint count;
    uint field;     // картинка поля расстояний (единицы сцены, внутри фигур < 0)
    float2 center;  // камера: точка сцены в центре экрана
    float aspect;
    float zoom;
    float3 ambient; // свет, который есть везде
};
[[vk::push_constant]] ConstantBuffer<Push> pc;

[[vk::binding(0, 0)]] Texture2D textures[];
[[vk::binding(1, 0)]] SamplerState samplers[2];

struct VertexOut
{
    float4 position : SV_Position;
    float2 screen : SCREEN;
};

[shader("vertex")]
VertexOut vs_main(uint vertex : SV_VulkanVertexID)
{
    float2 uv = float2((vertex << 1) & 2, vertex & 2);
    float2 ndc = uv * 2.0 - 1.0;
    VertexOut o;
    o.position = float4(ndc, 0.0, 1.0);
    o.screen = float2(ndc.x * pc.aspect, -ndc.y);
    return o;
}

// Поле в точке сцены p (между точками картинки значения смешиваются).
float field_at(float2 p)
{
    float2 s = (p - pc.center) * pc.zoom;
    float2 uv = float2(s.x / pc.aspect * 0.5 + 0.5, 0.5 - s.y * 0.5);
    return textures[pc.field].SampleLevel(samplers[0], uv, 0.0).r;
}

// Мягкая тень: доля источника, видная из p по направлению dir на расстоянии до dist.
// Шагаем по полю; h / t — насколько близко луч прошёл к фигуре относительно пройденного
// пути, softness задаёт размер источника (приём Иниго Килеса «soft shadows» для SDF).
float soft_shadow(float2 p, float2 dir, float dist, float softness, float step_min)
{
    float lit = 1.0;
    float t = step_min;
    for (int i = 0; i < 32 && t < dist; i++)
    {
        float h = field_at(p + dir * t);
        if (h < 0.0)
            return 0.0;
        lit = min(lit, h / (softness * t));
        t += max(h, step_min);
    }
    lit = saturate(lit);
    return lit * lit * (3.0 - 2.0 * lit);
}

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    float2 p = pc.center + input.screen / pc.zoom;
    uint w, h;
    textures[pc.field].GetDimensions(w, h);
    // Шаг не короче точки поля: меньше поле всё равно не различает.
    float step_min = 2.0 / (float(h) * pc.zoom);
    // Внутри фигур тень не ищем: их поверхность освещена (как крышка предмета сверху).
    bool inside = field_at(p) < 0.0;

    float3 light = pc.ambient;
    float glow = 0.0;
    for (uint i = 0; i < pc.count; i++)
    {
        Light l = pc.lights[i];
        float2 dir;
        float dist;
        float fade;
        if (l.kind == 0)
        {
            // Ближайшая точка лампы или трубки.
            float2 ab = l.b - l.a;
            float k = saturate(dot(p - l.a, ab) / max(dot(ab, ab), 1e-8));
            float2 v = l.a + ab * k - p;
            dist = length(v);
            if (dist >= l.reach)
                continue;
            dir = v / max(dist, 1e-6);
            float r = dist / l.reach;
            fade = (1.0 - r * r) * (1.0 - r * r);
            // Сама лампа светится: яркое пятно размером с пару пикселей поля.
            glow = max(glow, exp(-dist / (3.0 * step_min)));
        }
        else
        {
            dir = l.a;
            dist = l.reach;
            fade = 1.0;
        }
        float lit = inside ? 1.0 : soft_shadow(p, dir, dist, l.softness, step_min);
        light += l.color * fade * lit;
    }
    return float4(light, glow);
}

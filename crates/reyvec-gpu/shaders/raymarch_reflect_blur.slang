// Сцена «3D», проходы «отражения 3D: размытие» (Л2, docs/rays-further.md): размытые
// отражения шероховатых материалов. Отражение (raymarch_reflect.slang) у них посчитано
// резким и только в левом верхнем пикселе квадрата 2×2; здесь оно размывается двумя
// проходами — по строке (pc.flags бит 1 = 0), потом по столбцу (бит 1 = 1).
//
// Ширина размытия — как у конуса отражения: угол растёт с шероховатостью (до 0,5 рад при
// 1), пятно на месте попадания — угол × длина луча отражения, а на экране это пятно
// делится на ширину пикселя на расстоянии точки. Ближняя к зеркалу вещь отражается
// резче дальней, как в жизни. Не больше REFLECT_BLUR_MAX пикселей.
//
// Размытие с оглядкой на края: соседняя точка берётся, только если у неё тот же
// материал, почти та же глубина и сторона, — отражение пола не перетекает на ножку стола.
// Резкие пиксели (шероховатость 0 или не блестит) переписываются как есть.
//
// Тензорные ядра здесь не годятся: вес каждой соседней точки свой (по краям и ширине
// пятна), а умножение матриц даёт одно ядро на всю картинку и размыло бы через края.
//
// Константы: pc.gbuffer — буфер геометрии, pc.cone — что размывать (резкие отражения
// или итог первого прохода), pc.meshes — резкие отражения (по ним длина луча), pc.target
// — куда писать.

[[vk::binding(2, 0)]] [format("rgba16f")] RWTexture2D<float4> images[];

#include "include/space.slang"
#include "include/shade3d.slang"

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    return float4(0.0, 0.0, 0.0, 0.0);
}

static const float REFLECT_BLUR_MAX = 24.0;
static const float REFLECT_CONE = 0.5;
// Сколько точек по каждую сторону берёт проход: шире — с шагом больше пикселя.
static const int REFLECT_TAPS = 12;
static const uint BLUR_VERTICAL = 2;

uint material_at(float y)
{
    return min(uint(y + 0.5), max(pc.scene[3], 1) - 1);
}

// Точка сцены пикселя `q` с глубиной `t`.
float3 point_at(int2 q, int2 size, float t)
{
    float2 screen = float2((float(q.x) + 0.5) / float(size.x) * 2.0 - 1.0, 1.0 - (float(q.y) + 0.5) / float(size.y) * 2.0);
    Ray ray = screen_ray(screen);
    return ray.origin + ray.dir * t;
}

// Вес соседней точки `q` для пикселя в точке `p` (глубина `t`, нормаль `n`, материал
// `m`): тот же материал, почти та же плоскость (по расстоянию до плоскости пикселя, а не
// по глубине: на полу вдали глубина соседних строк сильно разная) и та же сторона.
float surface_weight(int2 q, int2 size, float3 p, float t, float3 n, uint m)
{
    if (any(q < 0) || any(q >= size))
        return 0.0;
    float4 g = gbuffer_load(q);
    if (g.x < 0.0 || material_at(g.y) != m)
        return 0.0;
    if (abs(dot(point_at(q, size, g.x) - p, n)) > 0.01 * t + 0.01)
        return 0.0;
    float facing = dot(oct_decode(g.zw), n);
    return facing > 0.8 ? 1.0 : 0.0;
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
    int2 size = int2(w, h);
    float4 here = textures[pc.cone].Load(int3(px, 0));
    float4 g = gbuffer_load(px);
    uint m = material_at(g.y);
    float rough = g.x < 0.0 ? 0.0 : material_rough(m);
    if (rough <= 0.0 || material_shine(m) <= 0.0)
    {
        images[pc.target][id.xy] = here;
        return;
    }
    // Длина луча отражения: у своего пикселя или у левого верхнего в квадрате 2×2.
    float4 sharp = textures[pc.meshes].Load(int3(px, 0));
    if (sharp.a <= 0.0)
        sharp = textures[pc.meshes].Load(int3(px & ~1, 0));
    float reach = sharp.a > 0.0 ? sharp.a : 2.0;
    // Ширина пикселя на метр пути: соседний луч экрана.
    float2 screen = float2((float(px.x) + 0.5) / float(w) * 2.0 - 1.0, 1.0 - (float(px.y) + 0.5) / float(h) * 2.0);
    Ray ray = screen_ray(screen);
    Ray next = screen_ray(screen + float2(2.0 / float(w), 0.0));
    float spread = length(next.dir - ray.dir);
    float radius = rough * REFLECT_CONE * min(reach, 20.0) / max(g.x * spread, 1e-5);
    // Не меньше полутора пикселей: пиксели без своего луча берут соседей.
    radius = clamp(radius, 2.0, REFLECT_BLUR_MAX);
    float stride = max(radius / float(REFLECT_TAPS), 1.0);
    int taps = int(ceil(radius / stride));
    float sigma = radius * 0.5;
    float3 n = oct_decode(g.zw);
    float3 p = ray.origin + ray.dir * g.x;
    bool vertical = (pc.flags & BLUR_VERTICAL) != 0;
    // По строке берётся чётная строка: в нечётных своих лучей нет.
    int2 base = vertical ? px : int2(px.x, px.y & ~1);
    int2 step = vertical ? int2(0, 1) : int2(1, 0);
    float3 sum = float3(0.0, 0.0, 0.0);
    float total = 0.0;
    for (int k = -taps; k <= taps; k++)
    {
        float off = float(k) * stride;
        int2 q = base + step * int(round(off));
        // По строке свои лучи только в чётных столбцах: точка — к ближайшему чётному
        // (иначе при шаге 2 нечётные пиксели не находили бы ни одной).
        if (!vertical)
            q.x &= ~1;
        float4 v = textures[pc.cone].Load(int3(clamp(q, int2(0, 0), size - 1), 0));
        if (v.a <= 0.0)
            continue;
        // Сходство поверхности — по самой точке `q` (у пикселя чётной строки по строке).
        float weight = surface_weight(q, size, p, g.x, n, m);
        weight *= exp(-0.5 * (off / sigma) * (off / sigma));
        sum += v.rgb * weight;
        total += weight;
    }
    images[pc.target][id.xy] = total > 1e-4 ? float4(sum / total, reach) : float4(0.0, 0.0, 0.0, 0.0);
}

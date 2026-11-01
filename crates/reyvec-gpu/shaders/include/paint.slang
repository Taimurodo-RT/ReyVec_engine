// Градиенты векторных фигур. Подключается в vector.slang и vector_cover.slang через
// #include; сам по себе не собирается (в папке include нет точек входа).
//
// Градиент считается в координатах фигуры, поэтому едет вместе с ней. Цвета между
// точками градиента смешиваются в sRGB (как в SVG и редакторах), в линейный цвет
// переводится уже результат.

struct Paint
{
    float4 inverse;         // обратное преобразование градиента: матрица 2×2 (a, b, c, d)
    float2 inverse_offset;  // и сдвиг
    uint kind;              // 0 линейный, 1 радиальный
    uint spread;            // 0 тянуть края, 1 отражать, 2 повторять
    float4 geometry;        // линейный: начало.xy, конец.xy; радиальный: центр.xy, фокус.xy
    float radius;           // радиальный: радиус круга
    uint first_stop;
    uint stop_count;
    uint pad;
};

struct GradientStop
{
    float4 color; // sRGB без умножения на прозрачность
    float offset;
    uint pad0;
    uint pad1;
    uint pad2;
};

// Номер закраски у фигур одного цвета.
static const uint NO_PAINT = 0xffffffffu;

float3 srgb_to_linear(float3 c)
{
    float3 low = c / 12.92;
    float3 high = pow((c + 0.055) / 1.055, float3(2.4));
    return float3(
        c.x <= 0.04045 ? low.x : high.x,
        c.y <= 0.04045 ? low.y : high.y,
        c.z <= 0.04045 ? low.z : high.z);
}

// Доля градиента в точке `p` (в координатах градиента).
float gradient_position(Paint g, float2 p)
{
    if (g.kind == 0)
    {
        float2 a = g.geometry.xy;
        float2 d = g.geometry.zw - a;
        float len2 = dot(d, d);
        return len2 > 0.0 ? dot(p - a, d) / len2 : 1.0;
    }
    // Радиальный с фокусом: луч из фокуса через точку пересекает окружность на
    // расстоянии λ·|p − f|; доля = 1 / λ. Без сдвига фокуса это |p − c| / r.
    float2 c = g.geometry.xy;
    float2 f = g.geometry.zw;
    float2 d = p - f;
    float2 e = f - c;
    float dd = dot(d, d);
    if (dd < 1e-20)
        return 0.0;
    float ed = dot(e, d);
    float disc = ed * ed - dd * (dot(e, e) - g.radius * g.radius);
    float lambda = (-ed + sqrt(max(disc, 0.0))) / dd;
    return lambda > 0.0 ? 1.0 / lambda : 1.0;
}

float apply_spread(uint spread, float t)
{
    if (spread == 1)
        return 1.0 - abs(frac(t * 0.5) * 2.0 - 1.0);
    if (spread == 2)
        return frac(t);
    return saturate(t);
}

// Линейный цвет с прозрачностью (без умножения) закраски `index` в точке фигуры `local`.
float4 paint_color(Paint* paints, GradientStop* stops, uint index, float2 local)
{
    Paint g = paints[index];
    if (g.stop_count == 0)
        return float4(0.0, 0.0, 0.0, 0.0);
    float2 p = float2(
        g.inverse.x * local.x + g.inverse.z * local.y,
        g.inverse.y * local.x + g.inverse.w * local.y) + g.inverse_offset;
    float t = apply_spread(g.spread, gradient_position(g, p));

    GradientStop prev = stops[g.first_stop];
    float4 srgb = prev.color;
    if (t > prev.offset)
    {
        for (uint i = 1; i < g.stop_count; i++)
        {
            GradientStop next = stops[g.first_stop + i];
            srgb = next.color;
            if (t <= next.offset)
            {
                float span = next.offset - prev.offset;
                float k = span > 0.0 ? (t - prev.offset) / span : 1.0;
                srgb = lerp(prev.color, next.color, k);
                break;
            }
            prev = next;
        }
    }
    return float4(srgb_to_linear(srgb.rgb), srgb.a);
}

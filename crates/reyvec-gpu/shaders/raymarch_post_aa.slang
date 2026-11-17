// Сцена «3D», проход «сглаживание 3D по картинке» (8.12а): дешёвая замена лучам краёв
// для слабых видеокарт. Читает освещённую картинку «цвет 3D» (raymarch_shade.slang) и
// смягчает ступеньки на краях по ней самой, одним кадром — без прошлых кадров (не TAA).
//
// Способ — как у известного сглаживания по яркости (FXAA): у пикселя, где яркость резко
// отличается от соседей, по соседям 3×3 решается, край горизонтальный или вертикальный;
// потом вдоль края ищутся оба его конца (где перепад яркости кончается). Чем ближе
// пиксель к концу ступеньки, тем сильнее он берёт цвет с другой стороны края: так из
// лесенки получается наклонная линия. Плюс мягкость по среднему 3×3 для тонких деталей
// в пиксель шириной. Края без перепада яркости (одинаковый цвет по обе стороны) не
// трогаются: их и не видно.
//
// Картинка — pc.target; пишет в свою картинку того же размера (SPACE_COLOR_FORMAT).

#include "include/space.slang"

// Яркость для поиска краёв: после грубого перевода в экран (корень), как видит глаз.
float luma(float3 c)
{
    return sqrt(dot(saturate(c), float3(0.299, 0.587, 0.114)));
}

// Перепад меньше этого (и меньше доли от самой яркой точки) — не край.
static const float CONTRAST_MIN = 0.0312;
static const float CONTRAST_SHARE = 0.125;
// Шаги поиска концов края (в пикселях): сначала по одному, потом длиннее.
static const uint SEARCH_STEPS = 12;
static const float SEARCH_STRIDE[SEARCH_STEPS] = { 1.0, 1.0, 1.0, 1.0, 1.0, 1.5, 2.0, 2.0, 2.0, 2.0, 4.0, 8.0 };
// Сила мягкости для деталей в пиксель шириной (0 — нет, 1 — полное среднее).
static const float SUBPIXEL = 0.75;

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    Texture2D img = textures[pc.target];
    SamplerState s = samplers[0];
    uint w, h;
    img.GetDimensions(w, h);
    int2 last = int2(w, h) - 1;
    float2 inv = 1.0 / float2(w, h);
    int2 p = int2(input.position.xy);
    float3 center = img.Load(int3(p, 0)).rgb;

    // Яркость пикселя и соседей крестом.
    float lc = luma(center);
    float ln = luma(img.Load(int3(clamp(p + int2(0, -1), 0, last), 0)).rgb);
    float ls = luma(img.Load(int3(clamp(p + int2(0, 1), 0, last), 0)).rgb);
    float lw = luma(img.Load(int3(clamp(p + int2(-1, 0), 0, last), 0)).rgb);
    float le = luma(img.Load(int3(clamp(p + int2(1, 0), 0, last), 0)).rgb);
    float lo = min(lc, min(min(ln, ls), min(lw, le)));
    float hi = max(lc, max(max(ln, ls), max(lw, le)));
    float range = hi - lo;
    if (range < max(CONTRAST_MIN, hi * CONTRAST_SHARE))
        return float4(center, 1.0);

    // Углы 3×3.
    float lnw = luma(img.Load(int3(clamp(p + int2(-1, -1), 0, last), 0)).rgb);
    float lne = luma(img.Load(int3(clamp(p + int2(1, -1), 0, last), 0)).rgb);
    float lsw = luma(img.Load(int3(clamp(p + int2(-1, 1), 0, last), 0)).rgb);
    float lse = luma(img.Load(int3(clamp(p + int2(1, 1), 0, last), 0)).rgb);

    // Мягкость для деталей в пиксель: насколько пиксель отличается от среднего 3×3.
    float average = (2.0 * (ln + ls + lw + le) + lnw + lne + lsw + lse) / 12.0;
    float sub = saturate(abs(average - lc) / range);
    sub = smoothstep(0.0, 1.0, sub);
    sub = sub * sub * SUBPIXEL;

    // Край горизонтальный (линия слева направо), если яркость сильнее меняется по
    // вертикали.
    float across_y = 2.0 * abs(ln - 2.0 * lc + ls) + abs(lnw - 2.0 * lw + lsw)
        + abs(lne - 2.0 * le + lse);
    float across_x = 2.0 * abs(lw - 2.0 * lc + le) + abs(lnw - 2.0 * ln + lne)
        + abs(lsw - 2.0 * ls + lse);
    bool horizontal = across_y >= across_x;

    // С какой стороны от пикселя проходит край: где перепад больше.
    float l1 = horizontal ? ln : lw;
    float l2 = horizontal ? ls : le;
    float g1 = abs(l1 - lc);
    float g2 = abs(l2 - lc);
    bool first = g1 >= g2;
    float gradient = 0.25 * max(g1, g2);
    float step_len = (horizontal ? inv.y : inv.x) * (first ? -1.0 : 1.0);
    float edge_luma = 0.5 * (lc + (first ? l1 : l2));

    // Точка на самом крае (между пикселем и соседом) и шаг вдоль края.
    float2 uv = (float2(p) + 0.5) * inv;
    float2 on_edge = uv;
    if (horizontal)
        on_edge.y += 0.5 * step_len;
    else
        on_edge.x += 0.5 * step_len;
    float2 along = horizontal ? float2(inv.x, 0.0) : float2(0.0, inv.y);

    // Поиск обоих концов края.
    float2 a = on_edge - along;
    float2 b = on_edge + along;
    float da = luma(img.SampleLevel(s, a, 0.0).rgb) - edge_luma;
    float db = luma(img.SampleLevel(s, b, 0.0).rgb) - edge_luma;
    bool done_a = abs(da) >= gradient;
    bool done_b = abs(db) >= gradient;
    for (uint k = 1; k < SEARCH_STEPS && !(done_a && done_b); k++)
    {
        if (!done_a)
        {
            a -= along * SEARCH_STRIDE[k];
            da = luma(img.SampleLevel(s, a, 0.0).rgb) - edge_luma;
            done_a = abs(da) >= gradient;
        }
        if (!done_b)
        {
            b += along * SEARCH_STRIDE[k];
            db = luma(img.SampleLevel(s, b, 0.0).rgb) - edge_luma;
            done_b = abs(db) >= gradient;
        }
    }

    // Насколько сдвинуть точку чтения поперёк края: у конца ступеньки — на полпикселя, в
    // её середине — никак.
    float dist_a = horizontal ? uv.x - a.x : uv.y - a.y;
    float dist_b = horizontal ? b.x - uv.x : b.y - uv.y;
    bool nearer_a = dist_a < dist_b;
    float dist = min(dist_a, dist_b);
    float offset = 0.5 - dist / (dist_a + dist_b);
    // Сдвиг нужен, только если у ближнего конца яркость уходит в другую сторону, чем у
    // пикселя (иначе пиксель — не на ступеньке этого края).
    bool center_darker = lc < edge_luma;
    bool right_turn = ((nearer_a ? da : db) < 0.0) != center_darker;
    offset = right_turn ? offset : 0.0;
    offset = max(offset, sub);

    float2 read = uv;
    if (horizontal)
        read.y += offset * step_len;
    else
        read.x += offset * step_len;
    return float4(img.SampleLevel(s, read, 0.0).rgb, 1.0);
}

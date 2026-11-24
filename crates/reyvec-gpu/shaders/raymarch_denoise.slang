// Сцена «3D», шумодав режима «качество» (путь ReSTIR, шаг 5): накопление по времени и
// SVGF — размытие с оглядкой на края, тем сильнее, чем больше шум. Чистится только свет:
// «путь 3D» делит его на цвет первой точки (сетка пола, текстуры не размываются), а
// «шумодав: сборка» умножает обратно и добавляет дымку. Края и лучи глаза — как в
// быстром режиме, памяти кадров у них нет (TAA нет).
//
// Три вида прохода (pc.flags & 3):
// 1 «шумодав: время» — свет пикселя смешивается с памятью этого же места в прошлом кадре
//   (камера прошлого кадра — в словах сцены, include/restir.slang). Память — буфер
//   pc.edges, два слоя по 10 слов на пиксель (чётный и нечётный кадр, номер — pc.meshes):
//   свет, моменты яркости, длина истории и материал, точка и нормаль. Из моментов — разброс (variance), им SVGF решает, насколько размывать.
// 2 «шумодав: размытие» — à-trous: 5×5 точек с шагом (pc.flags >> 8) пикселей; вес соседа —
//   та же плоскость, та же нормаль, тот же материал и яркость в пределах шума.
// 3 «шумодав: сборка» — свет × цвет первой точки (pc.shadows), дымка к небу.
//
// pc.gbuffer — буфер геометрии, pc.cone — вход, pc.target — выход.

[[vk::binding(2, 0)]] [format("rgba16f")] RWTexture2D<float4> images[];

#include "include/march.slang"
#include "include/shade3d.slang"
#include "include/shadow3d.slang"
#include "include/trace3d.slang"
#include "include/light3d.slang"
#include "include/restir.slang"

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    return float4(0.0, 0.0, 0.0, 0.0);
}

static const uint DENOISE_TEMPORAL = 1;
static const uint DENOISE_ATROUS = 2;
static const uint DENOISE_COMPOSE = 3;
static const uint HISTORY_WORDS = 10;
// Память не длиннее стольких кадров: свет меняется — картинка догоняет за ~1/3 секунды.
static const float HISTORY_MAX = 24.0;
// У зеркал (отражение не связано с поверхностью) память короче, размытия нет.
static const float MIRROR_HISTORY = 4.0;

uint material_id(float y)
{
    return min(uint(y + 0.5), max(pc.scene[3], 1) - 1);
}

// Зеркальное: блестит и почти не шероховато — его свет — отражение, размывать нельзя.
bool mirror_like(uint m)
{
    return material_shine(m) > 0.3 && material_rough(m) < 0.1;
}

float3 pixel_point(int2 q, uint w, uint h, float t, out float3 dir)
{
    float2 screen = float2((float(q.x) + 0.5) / float(w) * 2.0 - 1.0, 1.0 - (float(q.y) + 0.5) / float(h) * 2.0);
    Ray ray = screen_ray(screen);
    dir = ray.dir;
    return ray.origin + ray.dir * t;
}

uint history_at(uint layer, int2 px, uint w, uint h)
{
    return ((layer * h + uint(px.y)) * w + uint(px.x)) * HISTORY_WORDS;
}

void temporal(int2 px, uint w, uint h)
{
    float4 g = gbuffer_load(px);
    float3 light = textures[pc.cone].Load(int3(px, 0)).rgb;
    uint layer = pc.meshes & 1u;
    uint at = history_at(layer, px, w, h);
    if (g.x < 0.0)
    {
        pc.edges[at + 5] = 0u;
        images[pc.target][px] = float4(light, 0.0);
        return;
    }
    float3 dir;
    float3 p = pixel_point(px, w, h, g.x, dir);
    float3 n = oct_decode(g.zw);
    uint m = material_id(g.y);
    float lum = luminance(light);
    float3 old = float3(0.0, 0.0, 0.0);
    float2 moments = float2(0.0, 0.0);
    float len = 0.0;
    int2 prev;
    if (previous_pixel(p, w, h, PREV_HISTORY, prev))
    {
        uint was = history_at(layer ^ 1u, prev, w, h);
        uint meta = pc.edges[was + 5];
        float3 pos = float3(asfloat(pc.edges[was + 6]), asfloat(pc.edges[was + 7]), asfloat(pc.edges[was + 8]));
        float3 normal = oct_decode(unpack_unorm2(pc.edges[was + 9]) * 2.0 - 1.0);
        bool same = (meta & 0xffffu) > 0u && (meta >> 16) == m && abs(dot(pos - p, n)) < 0.02 + 0.01 * g.x
                    && dot(normal, n) > 0.9;
        if (same)
        {
            old = float3(asfloat(pc.edges[was]), asfloat(pc.edges[was + 1]), asfloat(pc.edges[was + 2]));
            moments = float2(asfloat(pc.edges[was + 3]), asfloat(pc.edges[was + 4]));
            len = float(meta & 0xffffu);
        }
    }
    len = min(len + 1.0, mirror_like(m) ? MIRROR_HISTORY : HISTORY_MAX);
    float alpha = 1.0 / len;
    float3 mixed = lerp(old, light, alpha);
    moments = lerp(moments, float2(lum, lum * lum), alpha);
    float variance = max(moments.y - moments.x * moments.x, 0.0);
    // Короткая память — разброс по моментам ещё не верен: считаем шум сильнее.
    if (len < 4.0)
        variance = max(variance, lum * lum) * (4.0 / len);
    pc.edges[at] = asuint(mixed.r);
    pc.edges[at + 1] = asuint(mixed.g);
    pc.edges[at + 2] = asuint(mixed.b);
    pc.edges[at + 3] = asuint(moments.x);
    pc.edges[at + 4] = asuint(moments.y);
    pc.edges[at + 5] = uint(len) | (m << 16);
    pc.edges[at + 6] = asuint(p.x);
    pc.edges[at + 7] = asuint(p.y);
    pc.edges[at + 8] = asuint(p.z);
    pc.edges[at + 9] = pack_unorm2(oct_encode(n) * 0.5 + 0.5);
    images[pc.target][px] = float4(mixed, variance);
}

void atrous(int2 px, uint w, uint h)
{
    float4 here = textures[pc.cone].Load(int3(px, 0));
    float4 g = gbuffer_load(px);
    uint m = material_id(g.y);
    if (g.x < 0.0 || mirror_like(m))
    {
        images[pc.target][px] = here;
        return;
    }
    int step = int((pc.flags >> 8) & 0xffu);
    float3 dir;
    float3 p = pixel_point(px, w, h, g.x, dir);
    float3 n = oct_decode(g.zw);
    float lum = luminance(here.rgb);
    float sigma = 4.0 * sqrt(max(here.a, 0.0)) + 1e-3;
    const float kernel[3] = { 0.375, 0.25, 0.0625 };
    float3 sum = float3(0.0, 0.0, 0.0);
    float variance = 0.0;
    float total = 0.0;
    for (int dy = -2; dy <= 2; dy++)
    {
        for (int dx = -2; dx <= 2; dx++)
        {
            int2 q = px + int2(dx, dy) * step;
            if (any(q < 0) || q.x >= int(w) || q.y >= int(h))
                continue;
            float4 gq = gbuffer_load(q);
            if (gq.x < 0.0 || material_id(gq.y) != m)
                continue;
            float3 dq;
            float3 pq = pixel_point(q, w, h, gq.x, dq);
            float plane = abs(dot(pq - p, n)) / (0.02 + 0.01 * g.x);
            float facing = pow(saturate(dot(oct_decode(gq.zw), n)), 64.0);
            float4 v = textures[pc.cone].Load(int3(q, 0));
            float weight = kernel[abs(dx)] * kernel[abs(dy)] * facing * exp(-plane - abs(luminance(v.rgb) - lum) / sigma);
            sum += v.rgb * weight;
            variance += v.a * weight * weight;
            total += weight;
        }
    }
    if (total <= 1e-6)
    {
        images[pc.target][px] = here;
        return;
    }
    images[pc.target][px] = float4(sum / total, variance / (total * total));
}

void compose(int2 px, uint w, uint h)
{
    float3 light = textures[pc.cone].Load(int3(px, 0)).rgb;
    float4 aux = textures[pc.shadows].Load(int3(px, 0));
    float3 dir;
    pixel_point(px, w, h, 1.0, dir);
    images[pc.target][px] = float4(lerp(light * aux.rgb, sky(dir), aux.a), 1.0);
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
    uint mode = pc.flags & 3u;
    if (mode == DENOISE_TEMPORAL)
        temporal(px, w, h);
    else if (mode == DENOISE_ATROUS)
        atrous(px, w, h);
    else
        compose(px, w, h);
}

// Сцена «3D», освещение в проходе «сцена» (этап 7.3): читает буфер геометрии, который
// записал проход «реймарчинг» (raymarch.slang), и красит точку: материал, сетка пола,
// солнце, свет неба, блик и дымка. Мягкие тени и затенение в углах — 7.4.

#include "include/space.slang"

[[vk::binding(0, 0)]] Texture2D textures[];

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

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    Ray ray = screen_ray(input.screen);
    float4 g = textures[pc.gbuffer].Load(int3(int2(input.position.xy), 0));
    float t = g.x;
    // Как меняется луч к соседнему пикселю. Считается до ветвлений: в разных ветках
    // соседи квадрата 2×2 могут не дать производную. Луч меняется плавно везде, а точка
    // попадания — нет (на краю формы сосед смотрит на другую поверхность).
    float3 dox = ddx(ray.origin), doy = ddy(ray.origin);
    float3 ddx_dir = ddx(ray.dir), ddy_dir = ddy(ray.dir);
    if (t < 0.0)
        return float4(sky(ray.dir), 1.0);
    float3 p = ray.origin + ray.dir * t;

    uint material = min(uint(g.y + 0.5), max(pc.scene[3], 1) - 1);
    uint m = pc.scene[2] + material * 4;
    float3 albedo = float3(asfloat(pc.scene[m]), asfloat(pc.scene[m + 1]), asfloat(pc.scene[m + 2]));
    bool grid_on = (pc.scene[m + 3] & MATERIAL_GRID) != 0;
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

    // Простой свет: солнце сбоку сверху и рассеянный свет неба. Тени — в 7.4.
    float3 sun = normalize(float3(0.5, 0.8, 0.35));
    float diffuse = saturate(dot(n, sun));
    float3 light = float3(1.0, 0.95, 0.85) * diffuse + float3(0.25, 0.30, 0.40) * (0.5 + 0.5 * n.y);
    float3 color = albedo * light;
    // Блик у форм (не у пола с сеткой).
    if (!grid_on)
    {
        float3 h = normalize(sun - ray.dir);
        color += 0.25 * pow(saturate(dot(n, h)), 48.0);
    }
    // Даль уходит в дымку неба (сетка не рябит у горизонта).
    float fog = 1.0 - exp(-0.0025 * t * t * 0.1);
    return float4(lerp(color, sky(ray.dir), saturate(fog)), 1.0);
}

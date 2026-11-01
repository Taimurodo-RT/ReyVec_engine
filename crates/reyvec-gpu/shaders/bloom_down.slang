// Свечение (bloom, задача 4.4), шаг вниз. Яркие места кадра размываются в цепочке всё
// меньших картинок (половина кадра, четверть, …), потом цепочка собирается обратно
// (bloom_up.slang) и добавляется к кадру при выводе (present.slang). Так свет от ярких
// мест мягко расходится вокруг, как в глазу или камере.
// Первый шаг (mode 0) освещает кадр сцены светом (как вывод) в половине кадра и
// оставляет только то, что ярче порога; следующие (mode 1) уменьшают вдвое фильтром
// из 13 выборок (приём Хорхе Хименеса, «Next Generation Post Processing in Call of
// Duty: Advanced Warfare»): он не мерцает при движении и не даёт квадратов.

struct Push
{
    uint source;    // картинка, которую уменьшаем (mode 0: кадр сцены)
    uint light;     // mode 0: картинка света (light.slang)
    uint mode;
    float threshold; // mode 0: с какой яркости начинается свечение
    float2 texel;   // размер точки цели в долях картинки (1 / размер)
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
    // Вершины (-1,-1), (3,-1), (-1,3): треугольник накрывает весь экран.
    float2 uv = float2((vertex << 1) & 2, vertex & 2);
    VertexOut o;
    o.position = float4(uv * 2.0 - 1.0, 0.0, 1.0);
    return o;
}

float3 at(float2 uv)
{
    return textures[pc.source].SampleLevel(samplers[0], uv, 0.0).rgb;
}

// Плавный порог: ниже threshold − knee ничего, выше threshold всё, что сверху, между ними
// плавный переход (без резкой границы свечения).
float3 bright_part(float3 c)
{
    float b = max(c.r, max(c.g, c.b));
    float knee = pc.threshold * 0.5;
    float soft = clamp(b - pc.threshold + knee, 0.0, 2.0 * knee);
    soft = soft * soft / (4.0 * knee + 1e-5);
    float part = max(soft, b - pc.threshold) / max(b, 1e-5);
    return c * part;
}

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    float2 uv = input.position.xy * pc.texel;
    if (pc.mode == 0)
    {
        // Выборка посередине между четырьмя пикселями кадра — их среднее.
        float3 scene = at(uv);
        float4 l = textures[pc.light].SampleLevel(samplers[0], uv, 0.0);
        float3 lit = scene * l.rgb + l.rgb * l.a * 0.6;
        return float4(bright_part(lit), 1.0);
    }
    uint w, h;
    textures[pc.source].GetDimensions(w, h);
    float2 s = 1.0 / float2(w, h);
    float3 a = at(uv + s * float2(-2.0, -2.0));
    float3 b = at(uv + s * float2(0.0, -2.0));
    float3 c = at(uv + s * float2(2.0, -2.0));
    float3 d = at(uv + s * float2(-2.0, 0.0));
    float3 e = at(uv);
    float3 f = at(uv + s * float2(2.0, 0.0));
    float3 g = at(uv + s * float2(-2.0, 2.0));
    float3 hh = at(uv + s * float2(0.0, 2.0));
    float3 i = at(uv + s * float2(2.0, 2.0));
    float3 j = at(uv + s * float2(-1.0, -1.0));
    float3 k = at(uv + s * float2(1.0, -1.0));
    float3 l = at(uv + s * float2(-1.0, 1.0));
    float3 m = at(uv + s * float2(1.0, 1.0));
    float3 sum = e * 0.125 + (a + c + g + i) * 0.03125 + (b + d + f + hh) * 0.0625
        + (j + k + l + m) * 0.125;
    return float4(sum, 1.0);
}

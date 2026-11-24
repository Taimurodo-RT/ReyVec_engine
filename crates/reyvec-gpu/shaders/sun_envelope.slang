// Кэш солнца (опыт Р2, crates/reyvec-gpu/src/sun_cache.rs): «запас» карты неподвижного.
// Карта глубин D (sun_bake.slang) говорит, где первая форма от солнца; запас E говорит,
// насколько точка может быть ниже соседей, чтобы их полутень её не задела:
//
//     E(x) ≤ min по точкам b ≠ x (D(b) + CONE · texel · |x − b|).
//
// Считается с запасом вниз (меньше настоящего — значит, осторожнее, чем надо): окна-
// квадраты растут в полтора раза за проход (шаги — sun_cache::envelope_steps). W₀ = D;
// проход с шагом s берёт меньшее W из девяти точек со сдвигом s и пишет W' — наименьшее
// в квадрате радиуса R + s. Всё, что в нём ближе R + 1 точки, уже учтено прошлыми
// проходами с меньшей добавкой, поэтому E = min(E, W' + CONE · texel · (R + 1)). Сама точка
// x попадает в первое окно с добавкой CONE · texel — её луч всё равно сверяет точно
// (sun_static_known в include/shadow3d.slang).

struct Push
{
    float* src;
    float* dst;
    float* env;
    uint side;
    uint step;
    // CONE · texel · (R + 1), метры.
    float add;
    // Первый проход: E ещё не записан.
    uint first;
};
[[vk::push_constant]] ConstantBuffer<Push> pc;

struct VertexOut
{
    float4 position : SV_Position;
};

[shader("vertex")]
VertexOut vs_main(uint vertex : SV_VulkanVertexID)
{
    VertexOut o;
    o.position = float4(0.0, 0.0, 0.0, 1.0);
    return o;
}

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    return float4(0.0, 0.0, 0.0, 0.0);
}

[shader("compute")]
[numthreads(8, 8, 1)]
void cs_main(uint3 id : SV_DispatchThreadID)
{
    uint side = pc.side;
    if (id.x >= side || id.y >= side)
        return;
    int r = int(pc.step);
    float low = 1e9;
    for (int dy = -1; dy <= 1; dy++)
    {
        for (int dx = -1; dx <= 1; dx++)
        {
            int x = int(id.x) + dx * r;
            int y = int(id.y) + dy * r;
            if (x >= 0 && y >= 0 && x < int(side) && y < int(side))
                low = min(low, pc.src[uint(y) * side + uint(x)]);
        }
    }
    uint at = id.y * side + id.x;
    pc.dst[at] = low;
    float before = pc.first != 0 ? 1e9 : pc.env[at];
    pc.env[at] = min(before, low + pc.add);
}

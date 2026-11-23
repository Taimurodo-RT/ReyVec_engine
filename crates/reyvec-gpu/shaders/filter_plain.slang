// Опыт Р4 («Р4» в docs/3d-beyond-triangles.md, crates/reyvec-gpu/src/tensor_bench.rs):
// размытие по Гауссу обычными ядрами — точка на поток, 2R + 1 проб вдоль строки
// (`pc.vertical` 0) или столбца (1). Пара для замера с filter_tensor.slang: тот же ответ
// (снаружи картинки — нули), только тензорные ядра считают его умножением матриц.
//
// Картинка — числа float по строкам (`pc.src`, `pc.dst`) размером width × height; веса —
// `pc.taps`, 2R + 1 чисел (R = pc.radius).

struct Push
{
    float* src;
    float* dst;
    float* taps;
    uint width;
    uint height;
    uint vertical;
    uint radius;
};
[[vk::push_constant]] ConstantBuffer<Push> pc;

// Файл обязан иметь пару для рисования (build.rs собирает её у всех шейдеров).
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
    if (id.x >= pc.width || id.y >= pc.height)
        return;
    int r = int(pc.radius);
    float sum = 0.0;
    for (int o = -r; o <= r; o++)
    {
        int x = int(id.x) + (pc.vertical == 0 ? o : 0);
        int y = int(id.y) + (pc.vertical == 0 ? 0 : o);
        if (x >= 0 && y >= 0 && x < int(pc.width) && y < int(pc.height))
            sum += pc.src[uint(y) * pc.width + uint(x)] * pc.taps[o + r];
    }
    pc.dst[id.y * pc.width + id.x] = sum;
}

// Опыт Р4 («Р4» в docs/3d-beyond-triangles.md, crates/reyvec-gpu/src/tensor_bench.rs):
// то же размытие по Гауссу, что filter_plain.slang, на тензорных ядрах
// (VK_KHR_cooperative_matrix). Картинка режется на квадраты M×M (M = 16 у NVIDIA; 8 —
// filter_tensor8.slang, для lavapipe в CI). Размытие вдоль строки — это умножение квадрата
// на матрицу весов: точка (i, j) = сумма по x точек строки i, умноженных на g(j − x).
// Соседи до R = 16 точек лежат в B = 16 / M квадратах слева и справа, поэтому ответ —
// сумма 2B + 1 умножений: In[d] × G[d], где G[d][k][j] = g(j − M·d − k), d от −B до B.
// Вдоль столбца — так же, только матрица весов слева: G[d]ᵀ × In[d]. Числа картинки и
// веса — half (на входе тензорных ядер), сумма — float.
//
// Матрица на волну (subgroup): у NVIDIA волна — 32 потока (одна на группу), у lavapipe —
// 8 (четыре на группу). Каждая волна считает TILES_PER_WAVE квадратов подряд по строке:
// матрицы весов кладутся в общую память группы один раз.

#ifndef TENSOR_M
#define TENSOR_M 16
#endif
static const int M = TENSOR_M;
static const int B = 16 / TENSOR_M;
static const uint BLOCKS = 2 * (16 / TENSOR_M) + 1;
static const uint CELLS = TENSOR_M * TENSOR_M;

using namespace linalg;

typealias MatA = CoopMat<half, MemoryScope.Subgroup, TENSOR_M, TENSOR_M, CoopMatMatrixUse.MatrixA>;
typealias MatB = CoopMat<half, MemoryScope.Subgroup, TENSOR_M, TENSOR_M, CoopMatMatrixUse.MatrixB>;
typealias MatC = CoopMat<float, MemoryScope.Subgroup, TENSOR_M, TENSOR_M, CoopMatMatrixUse.MatrixAccumulator>;

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

// Как TENSOR_GROUP и TILES_PER_WAVE в tensor_bench.rs.
static const uint GROUP = 32;
static const uint MAX_WAVES = 4; // волна не короче 8 потоков
static const uint TILES_PER_WAVE = 4;

groupshared half g_weights[BLOCKS * CELLS];
groupshared half g_image[MAX_WAVES][BLOCKS * CELLS];
groupshared float g_out[MAX_WAVES][CELLS];

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

float tap(int o)
{
    int r = int(pc.radius);
    return abs(o) <= r ? pc.taps[o + r] : 0.0;
}

[shader("compute")]
[numthreads(GROUP, 1, 1)]
void cs_main(uint3 group : SV_GroupID, uint thread : SV_GroupIndex)
{
    uint lanes = WaveGetLaneCount();
    uint waves = GROUP / lanes;
    uint wave = thread / lanes;
    uint lane = thread % lanes;
    bool vertical = pc.vertical != 0;
    // Матрицы весов: [d][строка][столбец]. Вдоль строки G[d][k][j] = g(j − M·d − k),
    // вдоль столбца — транспонированная: A[d][i][k] = g(i − M·d − k).
    for (uint e = thread; e < BLOCKS * CELLS; e += GROUP)
    {
        int d = int(e / CELLS) - B;
        int row = int((e % CELLS) / uint(M));
        int col = int(e % uint(M));
        int o = vertical ? row - M * d - col : col - M * d - row;
        g_weights[e] = half(tap(o));
    }
    uint across = pc.width / uint(M);
    uint tiles = across * (pc.height / uint(M));
    for (uint step = 0; step < TILES_PER_WAVE; step++)
    {
        uint tile = (group.x * waves + wave) * TILES_PER_WAVE + step;
        bool inside = tile < tiles;
        uint bx = tile % across;
        uint by = tile / across;
        // Соседние квадраты: B слева, сам, B справа (или сверху и снизу).
        for (uint e = lane; e < BLOCKS * CELLS; e += lanes)
        {
            int d = int(e / CELLS) - B;
            int x = int(bx) * M + int(e % uint(M)) + (vertical ? 0 : M * d);
            int y = int(by) * M + int((e % CELLS) / uint(M)) + (vertical ? M * d : 0);
            float v = 0.0;
            if (inside && x >= 0 && y >= 0 && x < int(pc.width) && y < int(pc.height))
                v = pc.src[uint(y) * pc.width + uint(x)];
            g_image[wave][e] = half(v);
        }
        GroupMemoryBarrierWithGroupSync();
        MatC sum = MatC(0.0);
        for (uint d = 0; d < BLOCKS; d++)
        {
            if (vertical)
            {
                MatA a = MatA.Load<CoopMatMatrixLayout.RowMajor>(__getAddress(g_weights[d * CELLS]), M);
                MatB b = MatB.Load<CoopMatMatrixLayout.RowMajor>(__getAddress(g_image[wave][d * CELLS]), M);
                sum = coopMatMulAdd<float, false>(a, b, sum);
            }
            else
            {
                MatA a = MatA.Load<CoopMatMatrixLayout.RowMajor>(__getAddress(g_image[wave][d * CELLS]), M);
                MatB b = MatB.Load<CoopMatMatrixLayout.RowMajor>(__getAddress(g_weights[d * CELLS]), M);
                sum = coopMatMulAdd<float, false>(a, b, sum);
            }
        }
        sum.Store<CoopMatMatrixLayout.RowMajor>(__getAddress(g_out[wave][0]), M);
        GroupMemoryBarrierWithGroupSync();
        if (inside)
        {
            for (uint e = lane; e < CELLS; e += lanes)
                pc.dst[(by * uint(M) + e / uint(M)) * pc.width + bx * uint(M) + e % uint(M)] = g_out[wave][e];
        }
        GroupMemoryBarrierWithGroupSync();
    }
}

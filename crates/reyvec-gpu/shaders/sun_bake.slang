// Сцена «3D», проход «кэш солнца» (опыт Р2, «Р2» в docs/3d-beyond-triangles.md; процессорная
// часть — crates/reyvec-gpu/src/sun_cache.rs): печёт карту тени солнца от неподвижных
// предметов. Как у Quake, свет неподвижного считается один раз и хранится, а не
// пересчитывается каждый кадр.
//
// Слова сцены (`pc.scene`) — отдельная сцена только из неподвижных предметов, не сцена
// кадра. Точка карты пускает луч сверху, от солнца, вдоль −SUN и находит первую форму; в
// карту пишется глубина этой точки −dot(p, SUN), как у карт солнца из сеток
// (mesh_shadow.slang), пусто — 1e9. Карту читает map_shadow (include/shadow3d.slang).
//
// Карта печётся квадратами TILE×TILE (pc.cone) по несколько за кадр: квадрат с углом
// (pc.gbuffer, pc.shadows) карты стороной pc.flags точек пишет в буфер `pc.edges` на место
// pc.target (TILE² слов на квадрат), оттуда копирование кладёт его в картинку карты.
// Обратная матрица карты — в pc.inv0..inv3: точка карты с глубиной 1 — ближе к солнцу.

#include "include/space.slang"

// Больше шагов на луч не делаем; не дошедший луч считает точкой тени, где остановился
// (такой луч скользит вдоль формы).
static const uint SUN_BAKE_STEPS = 512;
// Пусто: глубина больше любой точки сцены.
static const float SUN_EMPTY = 1e9;

float4 fs_main(VertexOut input) : SV_Target
{
    return float4(0.0, 0.0, 0.0, 0.0);
}

[shader("compute")]
[numthreads(8, 8, 1)]
void cs_main(uint3 id : SV_DispatchThreadID)
{
    uint tile = pc.cone;
    if (id.x >= tile || id.y >= tile)
        return;
    float side = float(pc.flags);
    float2 at = float2(float(pc.gbuffer + id.x), float(pc.shadows + id.y)) + 0.5;
    // Как uv в map_shadow: x вправо, y карты вниз.
    float2 s = float2(at.x / side * 2.0 - 1.0, 1.0 - at.y / side * 2.0);
    float3 from = unproject(s, 1.0);
    float3 to = unproject(s, 0.0);
    float length_ = length(to - from);
    float3 dir = (to - from) / max(length_, 1e-6);
    float depth = SUN_EMPTY;
    float t = 0.0;
    for (uint i = 0; i < SUN_BAKE_STEPS; i++)
    {
        if (t >= length_)
            break;
        float3 q = from + dir * t;
        // Плоскости (пол, стены) в карте не нужны: живой луч считает их формулой.
        float h = scene_near(q, 0.05, true, false).x;
        if (h < 0.001 + 0.0002 * t || i + 1 == SUN_BAKE_STEPS)
        {
            depth = -dot(q, SUN);
            break;
        }
        t += max(h, 0.001);
    }
    pc.edges[pc.target * tile * tile + id.y * tile + id.x] = asuint(depth);
}

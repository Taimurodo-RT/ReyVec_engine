// Чтение картинки «полигоны 3D» (7.18е2): общее для лучей (march.slang), поиска краёв
// (raymarch_edge_find.slang) и освещения с текстурами (shade3d.slang). Нужен space.slang.
#pragma once

// Картинка «полигоны 3D» упакована (pack_mesh, 7.18е2): тот же общий список картинок
// (binding 0, что и textures[]), но читается как целые числа — одним образцом
// (textures_u, include/textures.slang) или с образцами MSAA. slangc предупреждает о
// совпадении binding (E39001): так и задумано, ячейка одна, вид чтения — по флагам.
[[vk::binding(0, 0)]] Texture2DMS<uint4> textures_ums[];
// Картинка «цвет полигонов 3D» с образцами MSAA (8.14б2), та же ячейка binding 0.
[[vk::binding(0, 0)]] Texture2DMS<float4> textures_fms[];

// Номер картинки полигонов: младшие 16 бит pc.meshes. Старшие (8.14б2, при
// SPACE_FLAG_MESH_COLOR) — номер картинки их цвета.
uint mesh_slot()
{
    return pc.meshes & 0xffff;
}

// Точка полигонов для пикселя `px`. Луч края `sub` (0…3) берёт свой образец MSAA (если
// они есть), луч середины пикселя (`sub` < 0) — ближайшую из накрытых.
float4 mesh_sample(int2 px, int sub)
{
    if ((pc.flags & SPACE_FLAG_MESH_MS) == 0)
        return unpack_mesh(textures_u[mesh_slot()].Load(int3(px, 0)).xy);
    if (sub >= 0)
    {
        uint s = (pc.flags >> (MESH_SAMPLE_SHIFT + 2 * uint(sub))) & 3;
        return unpack_mesh(textures_ums[mesh_slot()].Load(px, int(s)).xy);
    }
    uint n = (pc.flags & SPACE_FLAG_MESH_X4) != 0 ? 4 : 2;
    float4 best = float4(-1.0, 0.0, 0.0, 0.0);
    for (uint s = 0; s < n; s++)
    {
        float4 m = unpack_mesh(textures_ums[mesh_slot()].Load(px, int(s)).xy);
        if (m.x >= 0.0 && (best.x < 0.0 || m.x < best.x))
            best = m;
    }
    return best;
}

// Цвет текстуры полигона (8.14б2) материала `material` в пикселе `px`: из картинки «цвет
// полигонов 3D». Луч края `sub` берёт свой образец, если он накрыт этим материалом;
// иначе (и у луча середины пикселя, `sub` < 0) — ближайший образец этого материала. Нет
// такого образца (луч края пересёк соседнюю грань складки, 8.2а) — белый: цвет материала
// как есть.
float3 mesh_tint(int2 px, int sub, float material)
{
    if ((pc.flags & SPACE_FLAG_MESH_COLOR) == 0)
        return float3(1.0, 1.0, 1.0);
    uint colors = pc.meshes >> 16;
    if ((pc.flags & SPACE_FLAG_MESH_MS) == 0)
        return textures[colors].Load(int3(px, 0)).rgb;
    int pick = -1;
    if (sub >= 0)
    {
        uint s = (pc.flags >> (MESH_SAMPLE_SHIFT + 2 * uint(sub))) & 3;
        float4 m = unpack_mesh(textures_ums[mesh_slot()].Load(px, int(s)).xy);
        if (m.x >= 0.0 && abs(m.y - material) < 0.5)
            pick = int(s);
    }
    if (pick < 0)
    {
        uint n = (pc.flags & SPACE_FLAG_MESH_X4) != 0 ? 4 : 2;
        float best = -1.0;
        for (uint s = 0; s < n; s++)
        {
            float4 m = unpack_mesh(textures_ums[mesh_slot()].Load(px, int(s)).xy);
            if (m.x >= 0.0 && abs(m.y - material) < 0.5 && (best < 0.0 || m.x < best))
            {
                best = m.x;
                pick = int(s);
            }
        }
    }
    if (pick < 0)
        return float3(1.0, 1.0, 1.0);
    return textures_fms[colors].Load(px, pick).rgb;
}

// Образцы пикселя `px` расходятся (7.18е2): часть накрыта, часть нет, разные материалы
// или разрыв по глубине. Образцы одной грани хранят одно и то же (глубина — у луча
// середины пикселя), поэтому щель уже пикселя внутри одной плоскости (проём между
// призмами с общей гранью) луч середины не видит — его «ближайший накрытый» её
// закрывает. Поиск краёв по такому признаку всё равно ставит пиксель в список, и лучи
// края берут каждый свой образец. Стык граней одной формы под малым углом сюда не
// попадает: у двух плоскостей, сходящихся в пикселе, глубина у его середины почти одна.
// `t` — попадание луча середины (буфер геометрии): если оно заметно ближе всех образцов,
// полигоны в пикселе закрыты другой формой, и их образцы не в счёт.
bool mesh_mixed(int2 px, float t)
{
    if ((pc.flags & SPACE_FLAG_MESH_MS) == 0)
        return false;
    uint n = (pc.flags & SPACE_FLAG_MESH_X4) != 0 ? 4 : 2;
    float4 m0 = unpack_mesh(textures_ums[mesh_slot()].Load(px, 0).xy);
    float near = m0.x;
    bool mixed = false;
    for (uint s = 1; s < n; s++)
    {
        float4 m = unpack_mesh(textures_ums[mesh_slot()].Load(px, int(s)).xy);
        if (m.x >= 0.0 && (near < 0.0 || m.x < near))
            near = m.x;
        if ((m.x < 0.0) != (m0.x < 0.0))
            mixed = true;
        else if (m0.x >= 0.0 && (m.y != m0.y || abs(m.x - m0.x) > 0.01 * m0.x + 0.01))
            mixed = true;
    }
    return mixed && !(t >= 0.0 && t < near - (0.01 * near + 0.01));
}

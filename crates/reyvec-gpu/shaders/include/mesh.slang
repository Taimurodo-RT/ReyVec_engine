// Чтение картинки «полигоны 3D» (7.18е2): общее для лучей (march.slang) и поиска краёв
// (raymarch_edge_find.slang). Нужен space.slang.

// Картинка «полигоны 3D» упакована (pack_mesh, 7.18е2): тот же общий список картинок
// (binding 0, что и textures[]), но читается как целые числа — одним образцом или
// с образцами MSAA. slangc предупреждает о совпадении binding (E39001): так и задумано,
// ячейка одна, вид чтения — по флагам.
[[vk::binding(0, 0)]] Texture2D<uint4> textures_u[];
[[vk::binding(0, 0)]] Texture2DMS<uint4> textures_ums[];

// Точка полигонов для пикселя `px`. Луч края `sub` (0…3) берёт свой образец MSAA (если
// они есть), луч середины пикселя (`sub` < 0) — ближайшую из накрытых.
float4 mesh_sample(int2 px, int sub)
{
    if ((pc.flags & SPACE_FLAG_MESH_MS) == 0)
        return unpack_mesh(textures_u[pc.meshes].Load(int3(px, 0)).xy);
    if (sub >= 0)
    {
        uint s = (pc.flags >> (MESH_SAMPLE_SHIFT + 2 * uint(sub))) & 3;
        return unpack_mesh(textures_ums[pc.meshes].Load(px, int(s)).xy);
    }
    uint n = (pc.flags & SPACE_FLAG_MESH_X4) != 0 ? 4 : 2;
    float4 best = float4(-1.0, 0.0, 0.0, 0.0);
    for (uint s = 0; s < n; s++)
    {
        float4 m = unpack_mesh(textures_ums[pc.meshes].Load(px, int(s)).xy);
        if (m.x >= 0.0 && (best.x < 0.0 || m.x < best.x))
            best = m;
    }
    return best;
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
    float4 m0 = unpack_mesh(textures_ums[pc.meshes].Load(px, 0).xy);
    float near = m0.x;
    bool mixed = false;
    for (uint s = 1; s < n; s++)
    {
        float4 m = unpack_mesh(textures_ums[pc.meshes].Load(px, int(s)).xy);
        if (m.x >= 0.0 && (near < 0.0 || m.x < near))
            near = m.x;
        if ((m.x < 0.0) != (m0.x < 0.0))
            mixed = true;
        else if (m0.x >= 0.0 && (m.y != m0.y || abs(m.x - m0.x) > 0.01 * m0.x + 0.01))
            mixed = true;
    }
    return mixed && !(t >= 0.0 && t < near - (0.01 * near + 0.01));
}

// Кэш света (путь ReSTIR, шаг 5б, по мотивам SHaRC — spatially hashed radiance cache): свет,
// отражённый точкой мира, хранится в клетке сетки-хэша. Перед подключением — restir_gi.slang.
//
// Зачем: путь пикселя после первого отскока шёл ещё 3 отскока, и в каждой точке — лучи
// тени; на 5080 это 30–45 мс. С кэшем путь в точке отскока берёт прямой свет как раньше
// (лучи тени к лампе и солнцу), а весь дальнейший отражённый свет — из клетки кэша, и
// обрывается. Кэш наполняют «учебные» пиксели (квадраты 8×8, примерно каждый 16-й, свои
// в каждом кадре): они ведут путь целиком и в каждой его точке добавляют в клетку, сколько света пришло
// после неё. Проход «кэш света: сводка» в начале кадра сводит добавленное в среднее по
// нескольким кадрам и освобождает клетки, которые давно не трогали.
//
// Клетка: точка мира, округлённая до шага (чем дальше от камеры, тем крупнее: на экране
// около CACHE_CELL пикселей, не мельче 3 см, степень двойки), и сторона нормали (одна из шести осей). Хэш клетки —
// номер ячейки таблицы (CACHE_SIZE ячеек, пробуем CACHE_PROBES подряд) и проверочное
// число. Ячейка — CACHE_WORDS слов: проверочное число (0 — пусто), сумма света за кадр
// (3 целых, × CACHE_FIXED), сколько добавлено, итог (RGB9E5), сколько кадров в итоге и
// возраст. Таблица — в буфере запасов (pc.edges) после запасов DI и GI.
//
// Свет в клетке — «что отражает поверхность дальше прямого света»: альбедо × пришедший
// отражённый свет. Его же добавляет путь в точке вместо продолжения.

static const uint CACHE_SIZE = 1u << 21;
static const uint CACHE_WORDS = 8;
static const uint CACHE_PROBES = 8;
static const float CACHE_FIXED = 1024.0;
// Свет одного добавления не ярче (иначе сумма переполнится): как предел кандидата GI.
static const float CACHE_CLAMP = 16.0;
// Итог — среднее по стольким последним кадрам с добавлениями (свет меняется — кэш догоняет).
static const uint CACHE_HISTORY = 32;
// Клетку без добавлений столько кадров — освобождаем.
static const uint CACHE_MAX_AGE = 120;
// Клетке веришь, когда в итоге не меньше стольких кадров.
static const uint CACHE_TRUST = 2;
// Клетка на экране — примерно столько пикселей в ширину: так у каждой клетки за кадр
// набирается десяток-другой добавлений при любом размере кадра.
static const float CACHE_CELL = 10.0;
// Каждый какой пиксель — учебный.
static const uint CACHE_TRAIN_EVERY = 16;

uint cache_base(uint w, uint h)
{
    return 2u * w * h * (RESTIR_WORDS + GI_WORDS);
}

// Клетка точки `p` с нормалью `n` при камере в `eye` (`spread` — угол пикселя): номер в
// таблице и проверочное число.
void cache_key(float3 p, float3 n, float3 eye, float spread, out uint index, out uint check)
{
    float d = length(p - eye);
    int level = int(floor(log2(max(d * spread * CACHE_CELL, 0.03))));
    float size = exp2(float(level));
    int3 c = int3(floor(p / size));
    float3 a = abs(n);
    uint axis = a.x > a.y && a.x > a.z ? (n.x > 0.0 ? 0u : 1u) : (a.y > a.z ? (n.y > 0.0 ? 2u : 3u) : (n.z > 0.0 ? 4u : 5u));
    uint k = hash(uint(c.x) * 73856093u ^ hash(uint(c.y) * 19349663u ^ hash(uint(c.z) * 83492791u ^ hash(uint(level + 64) * 6u + axis))));
    index = k & (CACHE_SIZE - 1u);
    check = hash(k ^ 0x9e3779b9u) | 1u;
}

uint cache_slot(uint base, uint index, uint probe)
{
    return base + ((index + probe) & (CACHE_SIZE - 1u)) * CACHE_WORDS;
}

// Свет клетки, если она есть и ей можно верить.
bool cache_find(uint base, float3 p, float3 n, float3 eye, float spread, out float3 light)
{
    light = float3(0.0, 0.0, 0.0);
    uint index, check;
    cache_key(p, n, eye, spread, index, check);
    for (uint i = 0; i < CACHE_PROBES; i++)
    {
        uint at = cache_slot(base, index, i);
        uint have = pc.edges[at];
        if (have == 0u)
            return false;
        if (have == check)
        {
            if ((pc.edges[at + 6] & 0xffffu) < CACHE_TRUST)
                return false;
            light = unpack_rgb9e5(pc.edges[at + 5]);
            return true;
        }
    }
    return false;
}

// Добавить в клетку свет `light`.
void cache_add(uint base, float3 p, float3 n, float3 eye, float spread, float3 light)
{
    if (any(isnan(light)) || any(isinf(light)))
        return;
    light = clamp(light, 0.0, CACHE_CLAMP);
    uint index, check;
    cache_key(p, n, eye, spread, index, check);
    for (uint i = 0; i < CACHE_PROBES; i++)
    {
        uint at = cache_slot(base, index, i);
        uint was;
        InterlockedCompareExchange(pc.edges[at], 0u, check, was);
        if (was == 0u || was == check)
        {
            uint3 q = uint3(light * CACHE_FIXED + 0.5);
            InterlockedAdd(pc.edges[at + 1], q.x);
            InterlockedAdd(pc.edges[at + 2], q.y);
            InterlockedAdd(pc.edges[at + 3], q.z);
            InterlockedAdd(pc.edges[at + 4], 1u);
            return;
        }
    }
}

// Сводка ячейки `entry` (проход «кэш света: сводка»): добавленное за кадр — в среднее,
// давно не тронутые — освободить; `clear` — буфер новый, всё стереть.
void cache_resolve(uint base, uint entry, bool clear)
{
    uint at = base + entry * CACHE_WORDS;
    if (clear)
    {
        for (uint k = 0; k < CACHE_WORDS; k++)
            pc.edges[at + k] = 0u;
        return;
    }
    if (pc.edges[at] == 0u)
        return;
    uint count = pc.edges[at + 4];
    uint state = pc.edges[at + 6];
    uint frames = state & 0xffffu;
    uint age = state >> 16;
    if (count > 0u)
    {
        float3 added = float3(float(pc.edges[at + 1]), float(pc.edges[at + 2]), float(pc.edges[at + 3]))
                       / (CACHE_FIXED * float(count));
        frames = min(frames + 1u, CACHE_HISTORY);
        float3 old = unpack_rgb9e5(pc.edges[at + 5]);
        pc.edges[at + 5] = pack_rgb9e5(lerp(old, added, 1.0 / float(frames)));
        age = 0u;
        pc.edges[at + 1] = 0u;
        pc.edges[at + 2] = 0u;
        pc.edges[at + 3] = 0u;
        pc.edges[at + 4] = 0u;
    }
    else
    {
        age++;
        if (age > CACHE_MAX_AGE)
        {
            // Освободить. Пробы соседних клеток идут дальше пустой ячейки не будут: это
            // ломает поиск тех, кто лёг за ней, — они просто наполнятся заново.
            for (uint k = 0; k < CACHE_WORDS; k++)
                pc.edges[at + k] = 0u;
            return;
        }
    }
    pc.edges[at + 6] = frames | (age << 16);
}

// Учебный ли пиксель `px` в кадре `frame`. Учатся целые квадраты 8×8 (группа запуска):
// видеокарта считает пиксели пачками по 32, и один учебный (длинный) путь в пачке держал
// бы всю пачку — ускорения не было бы.
bool cache_training(uint2 px, uint frame)
{
    uint2 g = px / 8u;
    return hash(g.x * 7919u + g.y * 104729u + frame * 15485863u) % CACHE_TRAIN_EVERY == 0u;
}

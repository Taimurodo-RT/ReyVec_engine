// Список пикселей края (7.6о, 7.19а, 8.11б): пишет «края 3D: поиск»
// (raymarch_edge_find.slang), читает «края 3D» (raymarch_edges.slang). Раскладка — как
// edge_list в crates/reyvec-gpu/src/demo.rs.
//
// Заголовок (EDGE_HEADER слов): [0] — сколько групп по 64 потока запустить, [1], [2] — 1
// (размеры запуска), [3] — сколько пикселей края всего (его читает панель; и те, что не
// вошли в список), [4] — ненадёжных с четырьмя лучами, [5] — надёжных с четырьмя, [6] —
// замер краёв и биты 8.2 (слово Demo::edge_list_word), [7] — ненадёжных с двумя лучами,
// [8] — надёжных с двумя, [9] — порог «края: два луча вдали» (метров на пиксель, число
// float; 0 — выключено), [10]–[12] — размеры запуска «края 3D: тени» (8.22: групп
// ненадёжных, 1, 1), дальше не заняты. «Ненадёжные» (7.19а) — те, кому тени из
// половинного размера не подходят: «края 3D» считает им свои лучи тени.
//
// Пиксели (x | y << 16) после заголовка, на 2·w·h мест: ненадёжные с четырьмя лучами от
// начала первой половины, надёжные с четырьмя — от её конца назад; с двумя — так же во
// второй половине. Группы «краёв 3D» идут в порядке EDGE_ORDER: ненадёжные (4, потом 2
// луча), потом надёжные; группа с четырьмя лучами берёт 16 пикселей, с двумя — 32.

static const uint EDGE_HEADER = 16;
static const uint EDGE_LISTS = 4;
static const uint EDGE_U4 = 0;
static const uint EDGE_R4 = 1;
static const uint EDGE_U2 = 2;
static const uint EDGE_R2 = 3;
static const uint EDGE_FAR_WORD = 9;
// Слово заголовка: сколько групп ненадёжных частей (их тени считает «края 3D: тени»).
static const uint EDGE_SHADOW_GROUPS_WORD = 10;
// Слово заголовка со счётом каждой части.
static const uint EDGE_COUNT_WORD[EDGE_LISTS] = { 4, 5, 7, 8 };
// Порядок групп в запуске «краёв 3D».
static const uint EDGE_ORDER[EDGE_LISTS] = { EDGE_U4, EDGE_U2, EDGE_R4, EDGE_R2 };

// Пикселей на группу части `k`.
uint edge_list_per(uint k)
{
    return k >= EDGE_U2 ? 32 : 16;
}

// Слово пикселя `j` части `k` в кадре из `wh` пикселей.
uint edge_list_at(uint k, uint j, uint wh)
{
    uint half_start = EDGE_HEADER + (k >= EDGE_U2 ? wh : 0);
    bool backward = k == EDGE_R4 || k == EDGE_R2;
    return backward ? half_start + wh - 1 - j : half_start + j;
}

// Замер по частям (8.1, слово 6 заголовка списка, как EdgeProbe в demo.rs; читают «края
// 3D» и «края 3D: тени»): проход пропускает одну часть работы, и по разнице времени на
// ПК видно, сколько она стоит. Картинка краёв при этом неверная.
static const uint EDGE_PROBE_NO_EXACT_SHADOWS = 1;
static const uint EDGE_PROBE_NO_SUB_RAYS = 2;
static const uint EDGE_PROBE_NO_SHADING = 3;
static const uint EDGE_PROBE_EMPTY = 4;
// Бит 8 того же слова (8.2): складки без доп. лучей. Слово 6 — замер (младшие 8 бит)
// и этот бит, как Demo::edge_list_word в demo.rs.
static const uint EDGE_CREASES = 0x100;
// Бит 9 (8.2б): точные доли тени ненадёжных пикселей только там, где рядом проходит
// граница тени (shadow_steady); остальные берутся из картинки теней.
static const uint EDGE_SHADOW_REUSE = 0x200;

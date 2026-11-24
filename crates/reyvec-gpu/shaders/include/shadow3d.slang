// Тени и затенение в углах точки сцены «3D» (этап 7.4): общее для прохода «тени 3D»
// (raymarch_shadow.slang, половинный размер) и краёв форм (7.6о, raymarch_edges.slang,
// точно для пикселя края). Перед подключением — space.slang.

#ifdef SPACE_RAY_QUERY
// Тени через RT-ядра (опыт Р1, crates/reyvec-gpu/src/rt.rs; «Р1» в
// docs/3d-beyond-triangles.md). Шейдер с этим вариантом (raymarch_shadow_rt.slang)
// собирается, только если видеокарта умеет ray query.
//
// Видеокарта сама ищет, через какие коробки предметов проходит луч тени, и луч шагает
// только по формам этих предметов. Мягкая тень — минимум k·h/t по точкам луча, где h —
// расстояние до ближайшей формы, то есть минимум по всем формам сразу. Поэтому её можно
// считать по каждому предмету отдельно и взять наименьшее: картинка почти та же (точки
// шагов у разных предметов свои, поэтому полутень совпадает не до бита). Полутень даёт
// только форма ближе t/k к точке луча, поэтому коробки раздуты: на отрезке луча `i`
// (до k · RAY_INFLATE[i]) — на RAY_INFLATE[i] метров, у каждого отрезка своё дерево.

// Как rt::RAY_INFLATE.
static const float RAY_INFLATE[3] = { 0.1, 0.5, 3.0 };
// Больше шагов на весь луч (по всем предметам) не делаем: у сетки — 64.
static const uint RAY_MAX_STEPS = 128;

// Где раздел коробок RT в словах сцены; 0 — его нет, тени идут по сетке отсева.
uint ray_section()
{
    uint at = pc.scene[7];
    return at == 0 ? 0 : pc.scene[at + MESH_RAY];
}

RaytracingAccelerationStructure ray_tlas(uint rt)
{
    // Адрес — двумя 32-битными словами: так не нужна возможность shaderInt64.
    uint2 a = uint2(pc.scene[rt], pc.scene[rt + 1]);
    return spirv_asm { result:$$RaytracingAccelerationStructure = OpConvertUToAccelerationStructureKHR $a };
}

// Отрезок луча внутри коробки с углами по словам `b` (6 чисел): x — вход, y — выход.
float2 box_span(uint b, float3 from, float3 inv)
{
    float3 lo = float3(asfloat(pc.scene[b]), asfloat(pc.scene[b + 1]), asfloat(pc.scene[b + 2]));
    float3 hi = float3(asfloat(pc.scene[b + 3]), asfloat(pc.scene[b + 4]), asfloat(pc.scene[b + 5]));
    float3 a = (lo - from) * inv;
    float3 c = (hi - from) * inv;
    float3 near_ = min(a, c);
    float3 far_ = max(a, c);
    return float2(max(max(near_.x, near_.y), near_.z), min(min(far_.x, far_.y), far_.z));
}

// Полутень от одного предмета `o` на отрезке луча [t, end]: те же правила шага и
// уточнения, что у soft_shadow, только расстояние — до одной этой формы. `h_of_planes` —
// вместо предмета плоскости сцены, посчитанные формулой.
void shadow_object(uint o, bool h_of_planes, float3 from, float3 dir, float t, float end, float k,
                   inout float light, inout uint steps)
{
    float last = 1e9;
    while (t < end && steps < RAY_MAX_STEPS)
    {
        steps++;
        count_step();
        float3 q = from + dir * t;
        float2 s;
        if (h_of_planes)
            s = float2(planes_distance_shadow(q), 0.0);
        else
        {
            uint winner;
            float second;
            s = scene_walk<1>(q, max(0.15, 2.0 * t / k), true, false, o, 1e9, winner, second);
        }
        float h = s.x;
        if (h >= 1e8)
            return;
        float y = h * h / (2.0 * last);
        float d = sign(h) * sqrt(max(h * h - y * y, 0.0));
        if (s.y >= 0.0)
            light = min(light, max(k * d / max(t - y, 1e-4), k * h / t - 1.0));
        last = s.y < 0.0 ? 1e9 : h;
        if (light < 0.002)
            return;
        t += (k * h > 2.0 * t) ? h : clamp(h, 0.02, 0.6);
    }
}

float soft_shadow_rt(float3 from, float3 dir, float far, float k, uint rt)
{
    count_ray();
    float light = 1.0;
    uint steps = 0;
    uint count = min(pc.scene[rt + 2], MAX_OBJECTS);
    uint boxes = rt + 4;
    float3 inv = 1.0 / select(abs(dir) > 1e-8, dir, float3(1e-8, 1e-8, 1e-8));
    // Плоскости (пол, стены) — формулой, как у soft_shadow; в деревьях их нет.
    if (!away_from_planes(from, dir))
        shadow_object(0, true, from, dir, 0.02, far, k, light, steps);
    RaytracingAccelerationStructure tlas = ray_tlas(rt);
    float lo = 0.02;
    for (uint i = 0; i < 3; i++)
    {
        if (lo >= far || light < 0.002)
            break;
        float hi = i == 2 ? far : min(far, k * RAY_INFLATE[i]);
        RayQuery<RAY_FLAG_SKIP_TRIANGLES> q;
        RayDesc r;
        r.Origin = from;
        r.Direction = dir;
        r.TMin = lo;
        r.TMax = hi;
        q.TraceRayInline(tlas, RAY_FLAG_NONE, 1u << i, r);
        while (q.Proceed())
        {
            uint o = q.CandidatePrimitiveIndex();
            if (o >= count)
                continue;
            float2 span = box_span(boxes + (i * count + o) * 6, from, inv);
            float t0 = max(span.x, lo);
            float t1 = min(span.y, hi);
            if (t0 < t1)
                shadow_object(o, false, from, dir, t0, t1, k, light, steps);
            if (light < 0.002)
                q.Abort();
        }
        lo = hi;
    }
    light = saturate(light);
    return light * light * (3.0 - 2.0 * light);
}
#endif

// Доля света от источника по направлению `dir` с расстояния `far`: 1 — свет виден
// целиком, 0 — закрыт. `k` — жёсткость края (больше — резче).
float soft_shadow(float3 from, float3 dir, float far, float k)
{
    float light = 1.0;
#ifdef SPACE_RAY_QUERY
    uint rt = ray_section();
    if (rt != 0)
        return soft_shadow_rt(from, dir, far, k, rt);
#endif
    float t = 0.02;
    float last = 1e9;
    // Луч, уходящий от всех плоскостей (от пола к солнцу или к лампе над полом), они не
    // заслонят (7.6и): шагаем без них, иначе у пола шаг не длиннее высоты над полом.
    // Их полутень теряется, только когда свет почти вдоль плоскости (под углом меньше
    // 1/k, около 5°).
    count_ray();
    bool planes = !away_from_planes(from, dir);
    for (int i = 0; i < 64; i++)
    {
        if (t >= far)
            break;
        count_step();
        float3 q = from + dir * t;
        // Запечённое поле (7.10в). Если уже по его нижней оценке полутени здесь нет
        // (k · h больше 2t), формы не считаем. Шаг — на само число поля, а не на оценку:
        // оно почти равно настоящему расстоянию, и точки луча ложатся почти туда же, куда
        // и без поля. Иначе полосы полутени сдвигаются, и там, где луч перестаёт шагать по
        // полю, у них выходят резкие края (пятна на стыке стенок полой коробки). Поэтому же
        // поле берём, только когда запас оценки (0.87 шага сетки) меньше трети расстояния:
        // так перешаг числа поля за настоящее расстояние мал.
        // Плоскости (пол, стены) — формулой, один раз на шаг: и для оценки поля, и вместо
        // их программ в переборе (7.19в).
        float planes_d = planes ? planes_distance_shadow(q) : 1e9;
        float estimate;
        float lower = min(field_lower(q, estimate), planes_d);
        if (k * lower > 2.0 * t && lower > 2.0 * (estimate - lower))
        {
            t += estimate;
            last = estimate;
            continue;
        }
        // Граница объекта вместо формы, только если она так далеко, что полутени не даёт:
        // k · h / t заметно больше 1 (см. scene_near).
        float2 s = scene_near_shadow(q, max(0.15, 2.0 * t / k), planes_d);
        float h = s.x;
        // Уточнение по двум последним шагам: меньше полос в полутени.
        float y = h * h / (2.0 * last);
        // Со знаком: точка внутри формы — полная тень. Без знака точка луча внутри формы
        // считалась светлой. Так было у точек пола под скруглённым низом коробки: подъём
        // начала луча над полом (shadow_part) уводит его внутрь скругления, и пол под ним
        // выходил освещённым, а в шаге от него — тёмным. На наклонной линии касания
        // это давало ряд чёрных точек («зубцы» под коробками, 7.11).
        float d = sign(h) * sqrt(max(h * h - y * y, 0.0));
        // Полутень — только от настоящих форм. Оценка по шару-границе и так не даёт
        // полутени (порог выше), а оценка по клетке сетки (7.6е) меньше настоящего
        // расстояния и дала бы ложную полутень вдали.
        //
        // Уточнение не опускается ниже k · h / t − 1 (7.10в): где k · h больше 2t, оно
        // давало ложную тень — тёмные крошки по краю касания предметов с полом и светлое
        // пятно на стыке стенок. Ниже этого предела точки вдали от полутени не темнят, а
        // у полутени (k · h около t) предел ни на что не влияет; переход плавный, без
        // ступеньки. Благодаря ему точки, которые поле пропускает, и правда ничего не дают.
        if (s.y >= 0.0)
            light = min(light, max(k * d / max(t - y, 1e-4), k * h / t - 1.0));
        // Уточнение верит, что прошлый шаг — настоящее расстояние. Расстояние до шара
        // меньше настоящего, и с ним следующая точка выходит темнее, чем есть: после
        // такого шага уточнение пропускаем.
        last = s.y < 0.0 ? 1e9 : h;
        if (light < 0.002)
            break;
        // Короткие шаги нужны только у полутени, где k · h / t близко к 1. Где пусто
        // (k · h больше 2t), шаг на всю пустоту: луч к солнцу над полем уходит за
        // несколько шагов, а не за 50 (7.6г).
        t += (k * h > 2.0 * t) ? h : clamp(h, 0.02, 0.6);
    }
    light = saturate(light);
    return light * light * (3.0 - 2.0 * light);
}

// Как view3d::AO_REACH в crates/reyvec-sdf.
static const float AO_REACH = 1.0;
// Насколько темнеют углы: затенение — доля открытых направлений в этой степени (у
// открытых мест картинка почти та же, что до 7.13).
static const float AO_STRENGTH = 2.0;
// Пол и другие плоскости закрывают пробу наполовину: освещённый пол сам отражает свет
// (свет снизу, GROUND в shade3d.slang), поэтому низ формы у пола не уходит в черноту.
static const float PLANE_SHARE = 0.5;

// Затенение в углах: 1 — вокруг пусто, меньше — рядом другие формы.
//
// Пять направлений: вдоль нормали (пробы на 4, 16 и 40 см) и четыре, наклонённые от неё
// на 42° (пробы на 16 и 40 см). Проба сравнивает расстояние до ближайшей формы d с тем,
// каким оно было бы над ровной плоскостью (plane = h · cos наклона): закрытость пробы
// (plane − d) / (2 · plane) — 0 над ровным местом, половина, если проба касается формы
// (направление вдоль стенки: половину его конуса стенка закрывает), 1 — проба глубоко
// внутри формы. Направление закрыто настолько, насколько закрыта самая закрытая его
// проба; затенение — доля открытых направлений.
//
// До 7.13 закрытость проб складывалась в среднее и умножалась на 2,2. Это мера близости
// форм, а не того, куда виден свет: у пола под скруглённым низом коробки все пробы
// рядом с ней, и выходил 1 % света там, где на деле видна почти половина неба (чёрный
// клин на снимке Романа). Пол у стенки получал около 0,1 вместо половины. До 7.6п пробы
// шли только вдоль нормали: на узком скруглении (край дыры коробки с вырезанным шаром)
// выходила чёрная полоса по кругу.
float ambient_occlusion(float3 p, float3 n)
{
    // Два направления поперёк нормали (без ветвлений, Duff и др. 2017).
    float s = n.z >= 0.0 ? 1.0 : -1.0;
    float a = -1.0 / (s + n.z);
    float b = n.x * n.y * a;
    float3 t1 = float3(1.0 + s * n.x * n.x * a, s * b, -s * n.x);
    float3 t2 = float3(b, s + n.y * n.y * a, -n.y);
    count_ray();
    float visible = 0.0;
    float blocked = 0.0;
    // Проба: 0..2 — вдоль нормали (близко, средне, далеко), 3..10 — по наклонным
    // направлениям средне и далеко.
    for (uint i = 0; i < 11; i++)
    {
        float3 dir = n;
        float h;
        if (i < 3)
            h = i == 0 ? 0.04 : (i == 1 ? 0.16 : 0.4);
        else
        {
            uint k = (i - 3) >> 1;
            float3 side = (k & 1) == 0 ? t1 : t2;
            dir = normalize(n + ((k & 2) == 0 ? 0.9 : -0.9) * side);
            h = ((i - 3) & 1) == 1 ? 0.4 : 0.16;
        }
        count_step();
        float plane = h * dot(dir, n);
        float3 q = p + dir * h;
        float estimate;
        float lower = field_lower(q, estimate);
        float planes_d = planes_distance(q);
        // Запечённое поле (7.10г): если уже его нижняя оценка не меньше `plane`, проба
        // открыта, и формы не считаем. Картинка от этого не меняется.
        float probe = 0.0;
        if (min(lower, planes_d) < plane)
        {
            // «Углы из поля»: дальние пробы берут само число поля вместо расчёта форм. В
            // режиме «треугольники» (8.20г) — и ближняя: поле с шагом 3 см почти не
            // отличается от форм уже в 4 см от стенки, а программы форм там самые дорогие.
            float d;
            bool from_field = (pc.scene[11] & SCENE_AO_FIELD_ONLY) != 0
                || (i > 0 && (pc.scene[11] & 1) != 0);
            if (from_field && estimate > -1e29)
                d = estimate;
            else
                // Порог `plane`: форма дальше него пробу не закрывает (by_shapes = 0),
                // и её точное расстояние не нужно — scene_near такие пропускает по
                // шару-границе, не считая программу. Картинка та же.
                d = scene_near(q, plane, true, false).x;
            // Объекты не из списка клетки отвечают её начальной оценкой: до 7.18д4 это был
            // размер клетки, у уровня с полем 0,3 м — меньше дальней пробы 0,4 м, и открытые
            // места выходили чуть темнее. Со свободным расстоянием клетки этого почти нет.
            // Дальше AO_REACH расстояние не различаем: так объекты, отсеянные по взгляду
            // (7.6к, они дальше этого от любой пробы), на углы не влияют.
            d = min(d, AO_REACH);
            float by_shapes = saturate((plane - d) / (2.0 * plane));
            float by_planes = PLANE_SHARE * saturate((plane - planes_d) / (2.0 * plane));
            probe = max(by_shapes, by_planes);
        }
        blocked = max(blocked, probe);
        // Последняя проба направления: вдоль нормали — 2, наклонные — 4, 6, 8, 10.
        if (i == 2 || (i > 3 && ((i - 3) & 1) == 1))
        {
            visible += 1.0 - blocked;
            blocked = 0.0;
        }
    }
    return pow(visible / 5.0, AO_STRENGTH);
}

// Размер лампы для полутени карт теней: 1/k теней SDF.
static const float MAP_PENUMBRA = 0.1;
// Полутень карт теней не шире стольких точек карты (столько же — округа поиска
// заслоняющего).
static const float MAP_SEARCH = 24.0;

// Расстояние от лампы до плоскости точки (через `rel` = точка − лампа, нормаль `n`) вдоль
// направления, которое на карте лампы приходится на место `q` (в точках квадрата лампы).
// С ним, а не с расстоянием до самой точки, сравниваются точки карты вокруг: плоская
// поверхность сбоку от лампы сама себя не заслоняет, сколько бы шире ни была округа
// (7.18е). Почти вдоль плоскости — расстояние до самой точки `flat`. Место за краем
// квадрата — его край, как и у map_gather: точки карты и сравнение — одно направление.
float map_reference(float2 q, float side, float f, float3 rel, float3 n, float flat)
{
    q = clamp(q, 0.5, side - 0.5 - 1.0 / 256.0);
    float2 ndc = float2(q.x / side * 2.0 - 1.0, 1.0 - q.y / side * 2.0);
    float3 dir = normalize(float3(ndc / f, -1.0));
    float c = dot(dir, n);
    if (c > -0.05)
        return flat;
    // Сверху — предел для плоскости почти вдоль луча; снизу не ограничиваем: меньшее
    // расстояние только придумало бы заслоняющее.
    return min(dot(rel, n) / c, 2.0 * flat);
}

// Четыре точки карты вокруг места `q` (в точках квадрата лампы `lo`) одним чтением.
// Порядок Gather: w — (0, 0), z — (1, 0), x — (0, 1), y — (1, 1) от угла `corner`.
float4 map_gather(Texture2D map, float2 q, float2 lo, float side, float2 inv, out float2 frac_)
{
    // Все четыре — в квадрате этой лампы, не у соседней.
    q = clamp(q, 0.5, side - 0.5 - 1.0 / 256.0);
    float2 b = q - 0.5;
    float2 corner = floor(b);
    frac_ = b - corner;
    return map.GatherRed(samplers[1], (lo + corner + 1.0) * inv);
}

// Карта тени (7.9б): доля света точки `p` (нормаль `n`) от треугольников. Лампа `square`
// (`sun` = false) — расстояние до лампы по её карте (перспектива, mesh_shadow.slang);
// солнце (`sun`, 8.20в, режим «треугольники» 3D) — квадраты с `square` (сразу после ламп):
// ближняя карта, а где точки на ней нет — дальняя; прямоугольная
// проекция вдоль солнца, в карте — глубина от солнца −(точка · SUN). Обе — одной функцией
// с одними циклами: каждый лишний цикл проб в шейдере «теней 3D» и «краёв 3D» сильно
// удлиняет их сборку (на lavapipe в CI — за предел времени). `inside` — точка на карте;
// вне её — без тени (лампа) или тень лучами (солнце).
//
// Без ступеней в полутени (7.18е): прежние 25 проб стояли квадратом вдоль осей карты, и
// прямой край тени вдоль строки (столбца) карты переключал всю строку проб разом — скачки
// по 1/5 (по 1/25 только у косого края). Теперь пробы стоят по повёрнутой решётке (у всех
// разные x и y, поэтому и такой край проходит через них по одной), а каждая проба — четыре
// точки карты одним Gather со смесью по положению между ними: доля света меняется плавно.
//
// Полутень как у теней SDF: свет — диск (лампа — радиуса d/10, солнце — угла 1/12),
// полутень на точке шире во столько раз, во сколько заслоняющее ближе к свету, чем к
// точке. Сначала среднее заслоняющих в округе (9 проб, 36 точек карты), потом 25 проб в
// повёрнутом квадрате такого размера. Сравнение — с плоскостью точки на месте пробы
// (map_reference у лампы, наклон `slope` у солнца): плоская поверхность сама себя не
// заслоняет.
// Глубина (солнце) или расстояние (лампа) плоскости точки на месте карты `q`.
float map_plane(bool sun, float2 q, float2 uv, float side, float2 slope, float here, float f,
                float3 rel, float3 n)
{
    if (sun)
        return here + dot(clamp(q, 0.5, side - 0.5) - uv, slope);
    return map_reference(q, side, f, rel, n, here);
}

static const float SUN_MAP_SOFT = 1.0 / 12.0;
// Сцена «3D» (8.20з): точка для карты сдвигается по нормали на столько точек карты. В
// карте — плоские треугольники сетки, а нормаль точки — по наклону самой формы: на
// скруглённых краях и на гранях наискосок к свету плоскость точки расходилась с картой, и
// поверхность затеняла сама себя («ёлочка» на гранях, чёрные клинья на краях крышек).
static const float MAP_NORMAL_OFFSET = 1.5;

// Точка `p` в пространстве карты по матрице из слов сцены с `m`.
float4 map_clip(uint m, float3 p)
{
    float4 clip = float4(0.0, 0.0, 0.0, 0.0);
    for (uint c = 0; c < 4; c++)
    {
        float4 column = float4(asfloat(pc.scene[m + 4 * c]), asfloat(pc.scene[m + 4 * c + 1]),
                               asfloat(pc.scene[m + 4 * c + 2]), asfloat(pc.scene[m + 4 * c + 3]));
        clip += column * (c < 3 ? p[c] : 1.0);
    }
    return clip;
}

float map_shadow(float3 p, float3 n, uint square, bool sun, out bool inside)
{
    inside = false;
    uint at = pc.scene[7];
    if (!sun && square >= pc.scene[at + 1])
        return 1.0;
    // Солнце: ближняя карта, если точка на ней, иначе дальняя (квадраты после ламп).
    uint cascades = sun ? min(pc.scene[at + MESH_SUN], SUN_CASCADES) : 1;
    uint m = at + MESH_SHADOW_PROJ + square * 16;
    float2 ndc = float2(2.0, 2.0);
    for (uint k = 0; k < cascades; k++)
    {
        if (sun)
            m = at + MESH_SUN_PROJ + k * 16;
        float4 clip = map_clip(m, p);
        if (clip.w <= 1e-4)
            return 1.0;
        ndc = clip.xy / clip.w;
        if (all(abs(ndc) <= (sun ? 0.98 : 0.995)))
        {
            square += sun ? k : 0;
            break;
        }
    }
    if (any(abs(ndc) > (sun ? 0.98 : 0.995)))
        return 1.0;
    inside = true;
    Texture2D map = textures[pc.flags >> 16];
    uint w, h;
    map.GetDimensions(w, h);
    float side = float(h);
    // Сцена «3D» (солнце и лампы, смотрящие вниз): точка над поверхностью на
    // MAP_NORMAL_OFFSET точек карты (у лампы точка карты к краю шире: 1 + tan² угла).
    bool scene3d = sun || (pc.scene[11] & SCENE_LAMP_MAPS_DOWN) != 0;
    if (scene3d)
    {
        float world;
        if (sun)
        {
            float3 r0 = float3(asfloat(pc.scene[m]), asfloat(pc.scene[m + 4]), asfloat(pc.scene[m + 8]));
            world = 2.0 / (length(r0) * side);
        }
        else
            world = length(p - read_lamp(square).at) * 5.5 / side * (1.0 + 7.56 * dot(ndc, ndc));
        p += n * (MAP_NORMAL_OFFSET * world);
        float4 moved = map_clip(m, p);
        if (moved.w <= 1e-4)
            return 1.0;
        ndc = clamp(moved.xy / moved.w, -1.0, 1.0);
    }
    float2 inv = 1.0 / float2(float(w), float(h));
    float2 lo = float2(float(square) * side, 0.0);
    float2 uv = float2(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5) * side;
    // Лампа: камера смотрит вдоль −Z без поворота (level3d.rs, lamp_view_proj), первый
    // элемент матрицы — 1 / tan половины угла; мир на точку карты — 2·d·tan 70° / side.
    float3 rel = float3(0.0, 0.0, 0.0);
    float f = asfloat(pc.scene[m]);
    float here;
    float texel;
    float bias;
    float2 slope = float2(0.0, 0.0);
    if (sun)
    {
        // Строки матрицы — оси карты в мире, сторона карты — 2 / |строки|.
        float3 row0 = float3(asfloat(pc.scene[m]), asfloat(pc.scene[m + 4]), asfloat(pc.scene[m + 8]));
        float3 row1 = float3(asfloat(pc.scene[m + 1]), asfloat(pc.scene[m + 5]), asfloat(pc.scene[m + 9]));
        float scale = length(row0);
        texel = 2.0 / (scale * side);
        here = -dot(p, SUN);
        // Плоскость точки: на сдвиг (a, b) по карте глубина меняется на
        // (a·right − b·up)·n · texel / (SUN·n).
        float facing = dot(SUN, n);
        if (facing > 0.05)
            slope = float2(dot(row0, n), -dot(row1, n)) / scale * texel / facing;
        // До 8.20з предел был 2 точки: на гранях наискосок к солнцу плоскость обрезалась.
        slope = clamp(slope, -6.0 * texel, 6.0 * texel);
        bias = 0.01 + 1.5 * texel;
    }
    else
    {
        rel = p - read_lamp(square).at;
        // Лампа сцены «3D» смотрит вниз (8.20в2): нормаль и `rel` — в оси её вида
        // (map_reference сравнивает нормаль с направлениями вида). Поворачиваются оба:
        // с одной повёрнутой нормалью rel·n выходил неверным, и плоскость точки — тоже
        // (освещённый пол у основания предметов темнел).
        if ((pc.scene[11] & SCENE_LAMP_MAPS_DOWN) != 0)
        {
            n = float3(n.x, -n.z, n.y);
            rel = float3(rel.x, -rel.z, rel.y);
        }
        here = length(rel);
        texel = 5.5 * here / side;
        bias = 0.02 + 0.01 * here;
    }
    float blockers = 0.0;
    float count = 0.0;
    for (int k = 0; k < 9; k++)
    {
        float2 g = float2(float(k % 3), float(k / 3)) - 1.0;
        float2 q = uv + float2(3.0 * g.x - g.y, g.x + 3.0 * g.y) * (MAP_SEARCH * 0.25);
        float2 between;
        float4 depth = map_gather(map, q, lo, side, inv, between);
        float reference = map_plane(sun, q, uv, side, slope, here, f, rel, n);
        float4 shut = 1.0 - step(reference - bias, depth);
        blockers += dot(shut, depth);
        count += dot(shut, float4(1.0, 1.0, 1.0, 1.0));
    }
    if (count <= 0.0)
        return 1.0;
    // Все 36 точек округи заслонены — точка в полной тени: полутень уже округи, и
    // в ней светлого не найти (8.20и: на заводе так большая часть тени, а 25 проб
    // фильтра — две трети работы карты).
    if (count >= 36.0)
        return 0.0;
    float blocker = blockers / count;
    float world = sun ? max(here - blocker, 0.0) * SUN_MAP_SOFT
                      : (here - blocker) / max(blocker, 0.05) * here * MAP_PENUMBRA;
    float radius = clamp(world / texel, 1.0, MAP_SEARCH);
    // Узкая полутень (до 4 точек карты) — 9 проб повёрнутым квадратом 3×3 вместо 25:
    // на таком размере пробы с четырьмя точками каждая и так перекрываются (8.20и).
    // Цикл тот же, меняется только число проб: лишний цикл удлиняет сборку шейдера.
    int across = radius <= 4.0 ? 3 : 5;
    int probes = across * across;
    float half_ = float(across / 2);
    float spread = radius / (float(across) * half_ + half_);
    float lit = 0.0;
    for (int k = 0; k < probes; k++)
    {
        float2 g = float2(float(k % across), float(k / across)) - half_;
        float2 q = uv + float2(float(across) * g.x - g.y, g.x + float(across) * g.y) * spread;
        float2 between;
        float4 depth = map_gather(map, q, lo, side, inv, between);
        float reference = map_plane(sun, q, uv, side, slope, here, f, rel, n);
        float4 open = step(reference - bias, depth);
        lit += lerp(lerp(open.w, open.z, between.x), lerp(open.x, open.y, between.x),
                    between.y);
    }
    return lit / float(probes);
}

// Кэш солнца (опыт Р2): жёсткость конуса, с которой испечён запас карты (как
// sun_cache::CONE). Живой луч тени солнца (k = 12) не тронут предметом, если тот не ближе
// к лучу, чем t / 12 на расстоянии t; конус круче (10 < √(12² − 1)) — с запасом.
static const float SUN_STATIC_CONE = 10.0;
// Запас по глубине, метры: карта печётся с точностью около миллиметра.
static const float SUN_STATIC_MARGIN = 0.004;

// Кэш солнца (опыт Р2): что карта неподвижного точно знает о свете солнца в точке `p`
// (начало луча тени). 1 — неподвижное точно не затеняет (ни тени, ни полутени: живой луч
// может шагать только по движущимся), 0 — точка точно в полной тени неподвижного, −1 —
// не знает (край тени, полутень, точка вне карты): нужен живой луч по всем предметам.
//
// В картинке карты две половины. Слева — глубина первой формы от солнца в точке карты
// (−dot(q, SUN), пусто — 1e9). Справа — запас: в точке x не больше, чем
// min по всем точкам b (глубина b + CONE · расстояние от x до b вбок), кроме самой x
// (sun_envelope.slang). Луч из точки на глубине here вдоль солнца проходит от
// заслоняющего b на расстоянии вбок L, и его полутени нет, если here − глубина b ≤
// CONE · L. Четыре ближних точки карты сверяются точно (их расстояние вбок известно), все
// остальные — по запасу ближней: от неё до точки луча меньше половины диагонали точки.
int sun_static_known(float3 p)
{
    uint m = pc.scene[7] + MESH_SUN_PROJ;
    float4 clip = map_clip(m, p);
    if (clip.w <= 1e-4)
        return -1;
    float2 ndc = clip.xy / clip.w;
    if (any(abs(ndc) > 0.98))
        return -1;
    Texture2D map = textures[pc.flags >> 16];
    uint w, h;
    map.GetDimensions(w, h);
    float side = float(h);
    float2 inv = 1.0 / float2(float(w), float(h));
    float3 row0 = float3(asfloat(pc.scene[m]), asfloat(pc.scene[m + 4]), asfloat(pc.scene[m + 8]));
    float texel = 2.0 / (length(row0) * side);
    float here = -dot(p, SUN);
    float2 b = float2(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5) * side - 0.5;
    float2 corner = clamp(floor(b), 0.0, side - 2.0);
    float2 f = b - corner;
    // Порядок Gather: w — (0, 0), z — (1, 0), x — (0, 1), y — (1, 1) от угла.
    float4 ax = float4(f.x, 1.0 - f.x, 1.0 - f.x, f.x);
    float4 ay = float4(1.0 - f.y, 1.0 - f.y, f.y, f.y);
    float4 lateral = sqrt(ax * ax + ay * ay) * texel;
    float4 depth = map.GatherRed(samplers[1], (corner + 1.0) * inv);
    float4 envelope = map.GatherRed(samplers[1], (corner + float2(side, 0.0) + 1.0) * inv);
    // Все четыре точки карты рядом заслонены заметно выше: луч идёт сквозь неподвижное.
    if (all(depth < here - 0.02))
        return 0;
    if (any(here - depth + SUN_STATIC_MARGIN > SUN_STATIC_CONE * lateral))
        return -1;
    float4 bound = envelope - SUN_STATIC_CONE * lateral;
    float best = max(max(bound.x, bound.y), max(bound.z, bound.w));
    return best >= here + SUN_STATIC_MARGIN ? 1 : -1;
}

// Одна из четырёх долей света точки `surface` (на поверхности, нормаль `n`, `t` —
// расстояние вдоль луча глаза): 0 — солнце, 1 — затенение в углах, 2 и 3 — две первые
// лампы. Порядок — как в картинке теней (x, y, z, w).
float shadow_part(float3 surface, float3 n, float t, uint part)
{
    // Над поверхностью: иначе луч тени сразу «упирается» в саму точку, и по формам идёт
    // рябь. Шагание останавливается с точностью 0,5 мм на метр пути, поэтому дальние
    // точки поднимаются выше.
    float3 p = surface + n * (0.01 + 0.002 * t);
    if (part == 1)
        return ambient_occlusion(p, n);
    // 2.5D-вид (7.9б): уровень в помещении, солнца нет.
    if (part == 0 && (pc.scene[11] & SCENE_NO_SUN) != 0)
        return 0.0;
    float3 dir = SUN;
    float far = 30.0;
    float k_hard = 12.0;
    if (part >= 2)
    {
        if (part - 2 >= min(lamp_count(), SHADOW_LAMPS))
            return 1.0;
        Lamp l = read_lamp(part - 2);
        float3 to = l.at - p;
        float distance = length(to);
        dir = to / max(distance, 1e-4);
        // Луч останавливается, не дойдя до лампы 35 см: лампочка-шарик вокруг лампы
        // не должна закрывать свою же лампу (а луч, прошедший рядом с ней, — давать
        // полутень кольцами).
        far = distance - LAMP_SHADOW_GAP;
        k_hard = 10.0;
        // Лампа, чей свет здесь всё равно не виден (меньше 0,3 % яркости), луча тени
        // не стоит.
        if (lamp_falloff(l, distance) * dot(n, dir) <= 0.003)
            return 1.0;
    }
    // Только там, куда свет падает: обратная сторона и так тёмная.
    if (dot(n, dir) <= 0.0 || far <= 0.0)
        return 1.0;
    // Треугольники заслоняют по картам: лампу уровня (7.9б) — по её карте, и формы SDF
    // как обычно; солнце и лампы в режиме «треугольники» 3D (8.20в) — по их картам, лучи
    // только вне карт. Вызов один на обе: шейдер не растёт.
    uint mesh_at = pc.scene[7];
    bool sun = part == 0;
    // Кэш солнца (опыт Р2): карта неподвижного решает только то, что знает точно; у края
    // тени и в полутени — живой луч по всем предметам, как без кэша.
    if (sun && (pc.scene[11] & SCENE_SUN_STATIC) != 0 && (pc.flags & SPACE_FLAG_SHADOW_MAP) != 0)
    {
        int known = sun_static_known(p);
        if (known == 0)
            return 0.0;
        g_skip_still = known > 0;
        float light = soft_shadow(p, dir, far, k_hard);
        g_skip_still = false;
        return light;
    }
    float by_map = 1.0;
    if ((pc.flags & SPACE_FLAG_SHADOW_MAP) != 0
        && (!sun || (mesh_at != 0 && pc.scene[mesh_at + MESH_SUN] != 0)))
    {
        bool inside;
        uint square = sun ? min(pc.scene[mesh_at + 1], SHADOW_LAMPS) : part - 2;
        by_map = map_shadow(p, n, square, sun, inside);
        if (inside && (sun || (pc.scene[11] & SCENE_LAMP_MAPS_DOWN) != 0))
            return by_map;
        if (by_map <= 0.0)
            return 0.0;
    }
    return by_map * soft_shadow(p, dir, far, k_hard);
}

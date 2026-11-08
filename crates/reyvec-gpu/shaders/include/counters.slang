// Счётчики работы лучей (7.18д2): сколько лучей, шагов, вычислений сцены, объектов,
// программ форм и чтений поля слоя сделал проход. Есть только в вариантах шейдеров
// `*_counted.slang` (там определён SPACE_COUNTERS); в обычных count_* пусты и
// исчезают при сборке, поэтому замер времени по обычным шейдерам честный.
// Сложение в буфер — counters_flush в space.slang. Раскладка — space_counters в
// crates/reyvec-gpu/src/demo.rs.

// Как COUNTERS, COUNTER_SPREAD и номера проходов в demo.rs.
static const uint COUNTERS = 8;
static const uint COUNTER_SPREAD = 32;
static const uint PASS_CONE = 0;
static const uint PASS_MARCH = 1;
static const uint PASS_SHADOWS = 2;
static const uint PASS_AO = 3;
static const uint PASS_EDGES = 4;
static const uint PASS_EDGE_SHADOWS = 5;

#ifdef SPACE_COUNTERS
// У каждого потока свои (в SPIR-V — Private): сигнатуры функций не меняются.
static uint counted_rays = 0;
static uint counted_steps = 0;
static uint counted_scene = 0;
static uint counted_normal = 0;
static uint counted_objects = 0;
static uint counted_forms = 0;
static uint counted_field = 0;
static uint counted_material = 0;
void count_ray() { counted_rays++; }
void count_step() { counted_steps++; }
void count_scene() { counted_scene++; }
void count_normal() { counted_normal++; }
void count_object() { counted_objects++; }
void count_form() { counted_forms++; }
void count_field() { counted_field++; }
void count_material() { counted_material++; }
void counters_reset()
{
    counted_rays = 0;
    counted_steps = 0;
    counted_scene = 0;
    counted_normal = 0;
    counted_objects = 0;
    counted_forms = 0;
    counted_field = 0;
    counted_material = 0;
}
#else
void count_ray() {}
void count_step() {}
void count_scene() {}
void count_normal() {}
void count_object() {}
void count_form() {}
void count_field() {}
void count_material() {}
void counters_reset() {}
#endif

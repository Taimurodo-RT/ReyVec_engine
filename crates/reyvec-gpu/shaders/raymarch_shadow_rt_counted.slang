// Проход «тени 3D» через RT-ядра (опыт Р1) со счётчиками лучей (7.18д2): как
// raymarch_shadow_rt.slang, но считает шаги и вычисления сцены (include/counters.slang).
#define SPACE_COUNTERS 1
#define SPACE_RAY_QUERY 1
#include "raymarch_shadow.slang"

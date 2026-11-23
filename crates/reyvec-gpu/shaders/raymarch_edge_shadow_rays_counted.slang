// Проход «raymarch_edge_shadow_rays» со счётчиками лучей (7.18д2): тот же шейдер, что
// raymarch_edge_shadow_rays.slang, но считает шаги и вычисления сцены
// (include/counters.slang). Движок берёт этот вариант, только пока на панели включены
// счётчики.
// Точка входа [shader("compute")] cs_main — из подключённого файла (build.rs ищет эту пометку).
#define SPACE_COUNTERS 1
#include "raymarch_edge_shadow_rays.slang"

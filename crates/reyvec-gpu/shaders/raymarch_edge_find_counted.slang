// Проход «raymarch_edge_find» со счётчиками краёв (8.11а): тот же шейдер, что raymarch_edge_find.slang, но
// считает пиксели края по причине и сколько из них почти не видно (count_edge). Движок
// берёт этот вариант, только пока на панели включены счётчики.
// Точка входа [shader("compute")] cs_main — из подключённого файла (build.rs ищет эту пометку).
#define SPACE_COUNTERS 1
#include "raymarch_edge_find.slang"

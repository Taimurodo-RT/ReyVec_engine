// Опыт Р4: filter_tensor.slang на матрицах 8×8 — для видеокарт, которые умножают только
// такие (lavapipe в CI). Ответ тот же, умножений на квадрат пять вместо трёх.
// Точки входа ([shader("compute")] cs_main и пара для рисования) — в подключённом файле.

#define TENSOR_M 8
#include "filter_tensor.slang"

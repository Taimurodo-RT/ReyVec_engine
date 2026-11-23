// Проход «тени 3D» через RT-ядра (опыт Р1): тот же шейдер, что raymarch_shadow.slang,
// но лучи тени ищут предметы в дереве коробок (soft_shadow_rt в include/shadow3d.slang).
// Движок собирает этот вариант, только если видеокарта умеет ray query, и берёт его, пока
// на панели включён флажок «Тени через RT-ядра (опыт)».
#define SPACE_RAY_QUERY 1
#include "raymarch_shadow.slang"

// Общее для проходов jump flooding (jfa_*.slang).

#ifndef REYVEC_JFA
#define REYVEC_JFA

// «Нет семени» в картинке семян (сдвиг до точки края): больше любой картинки поля и
// точно хранится в половинной точности.
static const float NO_SEED = 60000.0;

bool has_seed(float2 offset)
{
    return offset.x < 30000.0;
}

#endif

// Переход к белому при выводе кадра: общий для present.slang (2D и смешанный кадр) и
// raymarch_upscale.slang (прямой вывод сцены «3D», 8.16).

// Мягкий переход к белому: до SHOULDER яркость как есть (цвета векторов не меняются),
// выше плавно приближается к 1 и никогда её не перескакивает. Сжимается самый яркий
// канал, остальные в той же пропорции, поэтому оттенок сохраняется; очень яркое
// (в несколько раз ярче белого) постепенно белеет, как пересвет на фото.
static const float SHOULDER = 0.8;

float3 tonemap(float3 c)
{
    float m = max(c.r, max(c.g, c.b));
    if (m <= SHOULDER)
        return c;
    float over = (m - SHOULDER) / (1.0 - SHOULDER);
    float mapped = SHOULDER + (1.0 - SHOULDER) * (1.0 - exp(-over));
    float3 hue = c * (mapped / m);
    float white = saturate((m - 1.0) / 4.0);
    return lerp(hue, float3(mapped, mapped, mapped), white * 0.6);
}

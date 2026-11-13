// Лёгкий шум (дизеринг) перед выводом в окно: экран хранит 256 ступеней яркости, и без
// шума плавный тёмный переход виден ступенями (узкое место векторов 53). Подключается
// в present.slang, а для прямого вывода сцены «3D» в окно (7.20а) — в raymarch_upscale.slang
// и raymarch_shade.slang; сам по себе не собирается (в папке include нет точек входа).

float srgb_encode(float x)
{
    return x <= 0.0031308 ? x * 12.92 : 1.055 * pow(x, 1.0 / 2.4) - 0.055;
}

float srgb_decode(float x)
{
    return x <= 0.04045 ? x / 12.92 : pow((x + 0.055) / 1.055, 2.4);
}

// Шум для пикселя p в пределах ±1 ступени экрана (сумма двух равномерных — треугольное
// распределение: так ступени исчезают, а шум почти не заметен). Не меняется от кадра к
// кадру, чтобы не мерцать.
float noise(float2 p)
{
    float a = frac(52.9829189 * frac(dot(p, float2(0.06711056, 0.00583715))));
    float b = frac(52.9829189 * frac(dot(p + float2(17.0, 59.0), float2(0.00583715, 0.06711056))));
    return a + b - 1.0;
}

float3 dither(float3 c, float2 p)
{
    float n = noise(p) / 255.0;
    float3 e = float3(srgb_encode(saturate(c.r)), srgb_encode(saturate(c.g)),
                      srgb_encode(saturate(c.b)));
    e = saturate(e + n);
    return float3(srgb_decode(e.r), srgb_decode(e.g), srgb_decode(e.b));
}

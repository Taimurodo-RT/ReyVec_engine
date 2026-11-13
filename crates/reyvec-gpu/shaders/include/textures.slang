// Общий список картинок и сэмплеров (bindless, crates/reyvec-gpu/src/bindless.rs) для
// шейдеров сцены «3D» (7.18б). Подключается из sdf3d.slang, поэтому есть во всех
// шейдерах, где есть формы: примитив «поле слоя» читает свою картинку прямо в
// sdf_primitive. Объявлять `textures[]` и `samplers[]` в самих шейдерах сцены «3D»
// не нужно. `images[]` (binding 2) у каждого шейдера свои: формат записи разный.
//
// samplers[0] — линейный, края растягиваются (CLAMP_TO_EDGE); samplers[1] — ближайшая
// точка. Поле слоя читать только samplers[0]: у его картинки за краем нет повторов.

#ifndef REYVEC_TEXTURES
#define REYVEC_TEXTURES

[[vk::binding(0, 0)]] Texture2D textures[];
// Та же ячейка, но картинки из целых чисел: упакованные буфер геометрии (7.20б) и
// «полигоны 3D» (7.18е2). slangc предупреждает о совпадении binding (E39001): так и
// задумано, ячейка одна, вид чтения — по картинке.
[[vk::binding(0, 0)]] Texture2D<uint4> textures_u[];
[[vk::binding(1, 0)]] SamplerState samplers[2];

#endif

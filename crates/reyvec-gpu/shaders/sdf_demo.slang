// Сцена «свет» (этап 4). Пока (задачи 4.1–4.2) показывает библиотеку SDF-фигур
// include/sdf2d.slang: каждая точка экрана считает расстояния до фигур прямо в шейдере,
// без треугольников. Фон показывает само поле расстояний: тонкие линии через равные
// расстояния от фигур, как высоты на карте. Сверху ряд простых фигур, снизу операции.
// Та же сцена на процессоре — тест light_scene_matches_cpu в
// crates/reyvec-app/tests/reference.rs: менять вместе.

#include "include/light_demo.slang"

struct Push
{
    float time;    // секунды с запуска
    float aspect;  // ширина / высота
    float2 center; // точка сцены в центре экрана (камера)
    float zoom;    // приближение камеры
    float height;  // высота окна в пикселях
    uint field_texture; // картинка поля расстояний (4.2), если flags & 1
    uint flags;         // 1: линии фона по полю из картинки (jump flooding), а не точному;
                        // 2: сцена с лампами (светлый пол, свет умножит вывод);
                        // 4: без фигур, только пол (сцена «слизь и магия»)
};
[[vk::push_constant]] ConstantBuffer<Push> pc;

[[vk::binding(0, 0)]] Texture2D textures[];
[[vk::binding(1, 0)]] SamplerState samplers[2];

struct VertexOut
{
    float4 position : SV_Position;
    float2 screen : SCREEN; // точка экрана: x от -aspect до aspect, y от -1 до 1 (вверх)
};

[shader("vertex")]
VertexOut vs_main(uint vertex : SV_VulkanVertexID)
{
    // Вершины (-1,-1), (3,-1), (-1,3): треугольник накрывает весь экран.
    float2 uv = float2((vertex << 1) & 2, vertex & 2);
    float2 ndc = uv * 2.0 - 1.0;
    VertexOut o;
    o.position = float4(ndc, 0.0, 1.0);
    o.screen = float2(ndc.x * pc.aspect, -ndc.y);
    return o;
}

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    float2 p = pc.center + input.screen / pc.zoom;
    float pixel = 2.0 / (pc.height * pc.zoom);
    float t = pc.time;

    float field = 1e9;
    // Пол: в сцене с лампами светлый (его затемняет и красит свет), иначе тёмный.
    float3 color = (pc.flags & 2) != 0 ? float3(0.32, 0.33, 0.36) : float3(0.02, 0.025, 0.035);
    bool shapes = (pc.flags & 4) == 0;
    // Сначала поле: считаем расстояние до ближайшей фигуры, потом рисуем линии. Без фигур
    // их и не считаем (это дорого, узкое место света 1).
    DemoShapes all;
    if (shapes)
    {
        all = demo_shapes(p, t);
        for (int i = 0; i < DEMO_SHAPES; i++)
            field = min(field, all.shapes[i].d);
    }
    // Поле для линий фона: точное или из картинки, построенной jump flooding (задача 4.2).
    // Картинка в половинном разрешении, между её пикселями значения смешиваются.
    float lines_field = field;
    if ((pc.flags & 1) != 0)
    {
        float2 uv = input.position.xy / float2(pc.height * pc.aspect, pc.height);
        lines_field = textures[pc.field_texture].SampleLevel(samplers[0], uv, 0.0).r;
    }
    // Линии поля каждые 0,1 единицы сцены, тонкие (около пикселя), тускнеют с расстоянием.
    float spacing = 0.1;
    float line_d = abs(frac(lines_field / spacing + 0.5) - 0.5) * spacing;
    float fade = exp(-3.0 * max(lines_field, 0.0));
    color += float3(0.05, 0.08, 0.12) * fade * sdf_coverage(line_d - pixel * 0.5, pixel);
    // Ореол у края, чтобы было видно, что поле продолжается и снаружи фигур.
    color += float3(0.04, 0.05, 0.07) * exp(-12.0 * max(lines_field, 0.0));

    if (!shapes)
        return float4(color, 1.0);
    // Фигуры поверх поля, каждая со своим мягким краем в пиксель.
    for (int i = 0; i < DEMO_SHAPES; i++)
        color = lerp(color, all.shapes[i].color, sdf_coverage(all.shapes[i].d, pixel));
    // Тонкий светлый контур по краю (там, где расстояние около нуля).
    color = lerp(color, float3(1.0, 1.0, 1.0), 0.35 * sdf_coverage(abs(field) - pixel * 0.5, pixel));
    return float4(color, 1.0);
}

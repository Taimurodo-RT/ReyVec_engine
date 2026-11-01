// Сцена «свет» (этап 4). Пока (задача 4.1) показывает библиотеку SDF-фигур
// include/sdf2d.slang: каждая точка экрана считает расстояния до фигур прямо в шейдере,
// без треугольников. Фон показывает само поле расстояний: тонкие линии через равные
// расстояния от фигур, как высоты на карте. Сверху ряд простых фигур, снизу операции.

#include "include/sdf2d.slang"

struct Push
{
    float time;    // секунды с запуска
    float aspect;  // ширина / высота
    float2 center; // точка сцены в центре экрана (камера)
    float zoom;    // приближение камеры
    float height;  // высота окна в пикселях
};
[[vk::push_constant]] ConstantBuffer<Push> pc;

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

// Одна фигура сцены: расстояние и цвет.
struct Shape
{
    float d;
    float3 color;
};

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    float2 p = pc.center + input.screen / pc.zoom;
    float pixel = 2.0 / (pc.height * pc.zoom);
    float t = pc.time;

    Shape shapes[6];
    // Верхний ряд: круг, скруглённый прямоугольник, кривая Безье толщиной 0,04.
    shapes[0].d = sd_circle(p - float2(-1.2, 0.4), 0.28 + 0.03 * sin(t * 2.0));
    shapes[0].color = float3(0.95, 0.45, 0.2);
    shapes[1].d = sd_round_box(sdf_rotate(p - float2(-0.4, 0.4), t * 0.5),
                               float2(0.3, 0.18), 0.07);
    shapes[1].color = float3(0.25, 0.7, 0.95);
    float2 ctrl = float2(0.8, 0.75 + 0.25 * sin(t * 1.3));
    shapes[2].d = op_round(sd_bezier(p, float2(0.4, 0.2), ctrl, float2(1.25, 0.35)), 0.04);
    shapes[2].color = float3(0.95, 0.85, 0.3);

    // Нижний ряд: плавное объединение двух кругов (капли сливаются и расходятся),
    // прямоугольник с плавно вырезанным кругом, плавное пересечение круга и квадрата.
    float sep = 0.22 + 0.16 * sin(t * 1.1);
    float b1 = sd_circle(p - float2(-1.0 - sep, -0.45), 0.17);
    float b2 = sd_circle(p - float2(-1.0 + sep, -0.45), 0.2);
    shapes[3].d = op_smooth_union(b1, b2, 0.25);
    shapes[3].color = float3(0.4, 0.9, 0.45);
    float2 hole = float2(0.0 + 0.18 * cos(t * 0.9), -0.45 + 0.1 * sin(t * 0.9));
    shapes[4].d = op_smooth_subtract(sd_round_box(p - float2(0.0, -0.45), float2(0.3, 0.22), 0.04),
                                     sd_circle(p - hole, 0.15), 0.08);
    shapes[4].color = float3(0.85, 0.4, 0.9);
    float2 q = p - float2(1.0, -0.45);
    shapes[5].d = op_smooth_intersect(sd_circle(q, 0.3), sd_box(sdf_rotate(q, -t * 0.4), float2(0.22, 0.22)), 0.06);
    shapes[5].color = float3(0.95, 0.3, 0.4);

    float field = 1e9;
    float3 color = float3(0.02, 0.025, 0.035);
    // Сначала поле: считаем расстояние до ближайшей фигуры, потом рисуем линии.
    for (int i = 0; i < 6; i++)
        field = min(field, shapes[i].d);
    // Линии поля каждые 0,1 единицы сцены, тонкие (около пикселя), тускнеют с расстоянием.
    float spacing = 0.1;
    float line_d = abs(frac(field / spacing + 0.5) - 0.5) * spacing;
    float fade = exp(-3.0 * max(field, 0.0));
    color += float3(0.05, 0.08, 0.12) * fade * sdf_coverage(line_d - pixel * 0.5, pixel);
    // Ореол у края, чтобы было видно, что поле продолжается и снаружи фигур.
    color += float3(0.04, 0.05, 0.07) * exp(-12.0 * max(field, 0.0));

    // Фигуры поверх поля, каждая со своим мягким краем в пиксель.
    for (int i = 0; i < 6; i++)
        color = lerp(color, shapes[i].color, sdf_coverage(shapes[i].d, pixel));
    // Тонкий светлый контур по краю (там, где расстояние около нуля).
    color = lerp(color, float3(1.0, 1.0, 1.0), 0.35 * sdf_coverage(abs(field) - pixel * 0.5, pixel));
    return float4(color, 1.0);
}

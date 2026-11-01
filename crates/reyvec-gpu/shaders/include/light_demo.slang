// Фигуры сцены «свет» (задачи 4.1–4.2): общие для её картинки (sdf_demo.slang) и маски
// теней (light_mask.slang). Та же сцена на процессоре — тест light_scene_matches_cpu в
// crates/reyvec-app/tests/reference.rs: менять вместе.

#ifndef REYVEC_LIGHT_DEMO
#define REYVEC_LIGHT_DEMO

#include "sdf2d.slang"

static const int DEMO_SHAPES = 6;

// Одна фигура сцены: расстояние и цвет.
struct Shape
{
    float d;
    float3 color;
};

struct DemoShapes
{
    Shape shapes[DEMO_SHAPES];
};

// Фигуры в точке сцены p в момент t.
DemoShapes demo_shapes(float2 p, float t)
{
    DemoShapes out_shapes;
    // Верхний ряд: круг, скруглённый прямоугольник, кривая Безье толщиной 0,04.
    out_shapes.shapes[0].d = sd_circle(p - float2(-1.2, 0.4), 0.28 + 0.03 * sin(t * 2.0));
    out_shapes.shapes[0].color = float3(0.95, 0.45, 0.2);
    out_shapes.shapes[1].d = sd_round_box(sdf_rotate(p - float2(-0.4, 0.4), t * 0.5),
                               float2(0.3, 0.18), 0.07);
    out_shapes.shapes[1].color = float3(0.25, 0.7, 0.95);
    float2 ctrl = float2(0.8, 0.75 + 0.25 * sin(t * 1.3));
    out_shapes.shapes[2].d = op_round(sd_bezier(p, float2(0.4, 0.2), ctrl, float2(1.25, 0.35)), 0.04);
    out_shapes.shapes[2].color = float3(0.95, 0.85, 0.3);

    // Нижний ряд: плавное объединение двух кругов (капли сливаются и расходятся),
    // прямоугольник с плавно вырезанным кругом, плавное пересечение круга и квадрата.
    float sep = 0.22 + 0.16 * sin(t * 1.1);
    float b1 = sd_circle(p - float2(-1.0 - sep, -0.45), 0.17);
    float b2 = sd_circle(p - float2(-1.0 + sep, -0.45), 0.2);
    out_shapes.shapes[3].d = op_smooth_union(b1, b2, 0.25);
    out_shapes.shapes[3].color = float3(0.4, 0.9, 0.45);
    float2 hole = float2(0.0 + 0.18 * cos(t * 0.9), -0.45 + 0.1 * sin(t * 0.9));
    out_shapes.shapes[4].d = op_smooth_subtract(sd_round_box(p - float2(0.0, -0.45), float2(0.3, 0.22), 0.04),
                                     sd_circle(p - hole, 0.15), 0.08);
    out_shapes.shapes[4].color = float3(0.85, 0.4, 0.9);
    float2 q = p - float2(1.0, -0.45);
    out_shapes.shapes[5].d = op_smooth_intersect(sd_circle(q, 0.3), sd_box(sdf_rotate(q, -t * 0.4), float2(0.22, 0.22)), 0.06);
    out_shapes.shapes[5].color = float3(0.95, 0.3, 0.4);

    return out_shapes;
}

// Расстояние до ближайшей фигуры.
float demo_field(float2 p, float t)
{
    DemoShapes s = demo_shapes(p, t);
    float d = 1e9;
    for (int i = 0; i < DEMO_SHAPES; i++)
        d = min(d, s.shapes[i].d);
    return d;
}

#endif

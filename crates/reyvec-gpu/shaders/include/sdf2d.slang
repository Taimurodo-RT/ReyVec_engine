// Библиотека полей расстояний 2D (SDF). Функция фигуры возвращает расстояние от точки
// до её края: внутри отрицательное, снаружи положительное, на краю ноль.
// Зеркало на процессоре с тестами формул: crates/reyvec-sdf (те же имена и формулы,
// менять вместе).
// После плавных операций и обводки поле остаётся «снизу»: расстояние не больше
// настоящего, поэтому по нему можно безопасно шагать (тени не проскочат край).

#ifndef REYVEC_SDF2D
#define REYVEC_SDF2D

// Круг радиуса r с центром в нуле.
float sd_circle(float2 p, float r)
{
    return length(p) - r;
}

// Прямоугольник с центром в нуле и половинами сторон half.
float sd_box(float2 p, float2 half_size)
{
    float2 d = abs(p) - half_size;
    return length(max(d, 0.0)) + min(max(d.x, d.y), 0.0);
}

// Прямоугольник со скруглёнными углами (внешний размер как у sd_box).
float sd_round_box(float2 p, float2 half_size, float radius)
{
    float r = max(min(radius, min(half_size.x, half_size.y)), 0.0);
    return sd_box(p, half_size - r) - r;
}

// Отрезок от a до b (без толщины; толстая линия — op_round от него).
float sd_segment(float2 p, float2 a, float2 b)
{
    float2 pa = p - a;
    float2 ba = b - a;
    float len2 = dot(ba, ba);
    float h = len2 > 0.0 ? clamp(dot(pa, ba) / len2, 0.0, 1.0) : 0.0;
    return length(pa - ba * h);
}

// Квадратичная кривая Безье a → c с управляющей точкой b. Точное расстояние без знака
// (ближайшая точка из кубического уравнения, формула Иниго Килеса).
float sd_bezier(float2 p, float2 a, float2 b, float2 c)
{
    float2 aa = b - a;
    float2 bb = a - 2.0 * b + c;
    float2 cc = aa * 2.0;
    float2 d = a - p;
    float bb2 = dot(bb, bb);
    if (bb2 < 1e-10)
        return sd_segment(p, a, c);
    float kk = 1.0 / bb2;
    float kx = kk * dot(aa, bb);
    float ky = kk * (2.0 * dot(aa, aa) + dot(d, bb)) / 3.0;
    float kz = kk * dot(d, aa);
    float pp = ky - kx * kx;
    float p3 = pp * pp * pp;
    float q = kx * (2.0 * kx * kx - 3.0 * ky) + kz;
    float h = q * q + 4.0 * p3;
    float res;
    if (h >= 0.0)
    {
        h = sqrt(h);
        float2 x = (float2(h, -h) - q) / 2.0;
        float2 uv = sign(x) * pow(abs(x), float2(1.0 / 3.0));
        float t = clamp(uv.x + uv.y - kx, 0.0, 1.0);
        float2 v = d + (cc + bb * t) * t;
        res = dot(v, v);
    }
    else
    {
        float z = sqrt(-pp);
        float v = acos(clamp(q / (pp * z * 2.0), -1.0, 1.0)) / 3.0;
        float m = cos(v);
        float n = sin(v) * 1.7320508;
        float t0 = clamp((m + m) * z - kx, 0.0, 1.0);
        float t1 = clamp((-n - m) * z - kx, 0.0, 1.0);
        float2 v0 = d + (cc + bb * t0) * t0;
        float2 v1 = d + (cc + bb * t1) * t1;
        res = min(dot(v0, v0), dot(v1, v1));
    }
    return sqrt(res);
}

// Объединение, вырезание b из a, пересечение.
float op_union(float a, float b) { return min(a, b); }
float op_subtract(float a, float b) { return max(a, -b); }
float op_intersect(float a, float b) { return max(a, b); }

// Плавное объединение: фигуры сливаются, как капли; k — ширина перехода.
float op_smooth_union(float a, float b, float k)
{
    if (k <= 0.0)
        return min(a, b);
    float h = max(k - abs(a - b), 0.0) / k;
    return min(a, b) - h * h * k * 0.25;
}

// Плавное вырезание b из a и плавное пересечение.
float op_smooth_subtract(float a, float b, float k) { return -op_smooth_union(-a, b, k); }
float op_smooth_intersect(float a, float b, float k) { return -op_smooth_union(-a, -b, k); }

// Скругление (толщина линии) и кольцо по краю толщиной 2 * half_width.
float op_round(float d, float r) { return d - r; }
float op_onion(float d, float half_width) { return abs(d) - half_width; }

// Поворот точки на -angle: так фигура поворачивается на angle.
float2 sdf_rotate(float2 p, float angle)
{
    float s = sin(angle);
    float c = cos(angle);
    return float2(c * p.x + s * p.y, -s * p.x + c * p.y);
}

// Доля пикселя под фигурой: плавный край шириной в пиксель (pixel — размер пикселя в
// единицах сцены). SDF-фигуры сглаживаются сами, без MSAA, при любом приближении.
float sdf_coverage(float d, float pixel)
{
    return clamp(0.5 - d / max(pixel, 1e-12), 0.0, 1.0);
}

#endif

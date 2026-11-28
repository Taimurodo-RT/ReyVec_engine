// Частицы на видеокарте (6.6): пыль, искры, брызги.
//
// Частицы лежат в кольцевом буфере. Каждый кадр вычислительный шейдер `cs_main` проходит
// по живым частицам: новые (их процессор пишет в буфер загрузки) встают на свои места,
// остальные летят, падают, тормозят о воздух и отскакивают от того же, обо что
// ударяется герой (9.5): препятствий настоящей формы у экрана, разложенных по клеткам
// сетки (слова `world`, их раскладку описывает `particle_world.rs`). Потом пара
// `vs_main`/`fs_main` рисует каждую частицу мягким кружком. Зеркало расчёта на
// процессоре: `particle_world.rs`, `collide` (тесты сверяют поведение).

struct Particle
{
    float2 pos;
    float2 vel;
    float life;      // сколько ещё живёт, секунды (0 и меньше — мёртвая)
    float max_life;
    float size;      // поперечник кружка, единицы сцены
    float gravity;   // доля тяжести сцены (0 — летит прямо)
    float4 color;    // линейный цвет, может быть ярче 1 (искры светятся), альфа
    float drag;      // торможение о воздух, доля скорости за секунду
    float bounce;    // отскок: доля скорости поперёк поверхности (0 — прилипает)
    uint flags;      // PARTICLE_COLLIDE, PARTICLE_SHRINK, PARTICLE_GLOW
    uint pad;
};

static const uint PARTICLE_COLLIDE = 1;
static const uint PARTICLE_SHRINK = 2;
// Светится сама (искры): рисуется не в кадр сцены, который потом умножается на свет
// уровня, а поверх готового освещённого кадра при выводе.
static const uint PARTICLE_GLOW = 4;

struct Push
{
    Particle* particles;
    Particle* emitted;   // новые частицы этого кадра
    uint* world;         // мир для частиц: заголовок, препятствия, вершины, клетки, номера
    uint capacity;       // мест в кольце
    uint start;          // первое место живого отрезка
    uint count;          // сколько мест от start (живые и новые)
    uint head;           // куда встаёт первая новая частица
    uint emit_count;
    uint world_on;       // 0 — препятствий нет
    float dt;
    float gravity;       // тяжесть сцены, единиц в секунду за секунду
    // Для рисования: камера.
    float2 center;
    float aspect;
    float zoom;
    uint glow_pass;      // 1 — рисуются только светящиеся, при выводе; 0 — остальные
    uint build_list;     // 1 — последний шаг кадра: собрать списки живых для рисования
    // Списки для рисования (6.6б): два набора параметров непрямого рисования (по 4
    // числа: вершин, частиц, 0, 0), потом номера мест живых обычных частиц (capacity
    // штук), потом номера мест живых светящихся. Рисуются только живые: мёртвые места
    // кольца и чужой вид частиц не стоят ничего.
    uint* draw;
};

static const uint LIST_NORMAL = 8;
[[vk::push_constant]] ConstantBuffer<Push> pc;

static const float EPS = 1e-4;

// Мир для частиц (9.5), как `particle_world.rs`.
static const uint OBSTACLE_WORDS = 16;
static const uint KIND_BOX = 0;
static const uint KIND_CAPSULE = 1;
static const uint FLAG_ONEWAY = 1;

uint W(uint i) { return pc.world[i]; }
float F(uint i) { return asfloat(pc.world[i]); }
float2 F2(uint i) { return float2(F(i), F(i + 1)); }

float sgn(float x) { return x < 0.0 ? -1.0 : 1.0; }

float2 safe_normalize(float2 a)
{
    float l = length(a);
    return l > 1e-12 ? a / l : float2(0.0, 1.0);
}

float cross2(float2 a, float2 b) { return a.x * b.y - a.y * b.x; }

// Клетка сетки, где точка (0xffffffff — вне сетки).
uint cell_index(float2 p)
{
    float2 o = F2(0);
    float cell = F(2);
    uint nx = W(3);
    uint ny = W(4);
    float x = floor((p.x - o.x) / cell);
    float y = floor((p.y - o.y) / cell);
    if (x < 0.0 || y < 0.0 || x >= float(nx) || y >= float(ny))
        return 0xffffffffu;
    return uint(y) * nx + uint(x);
}

// Вход пути (от `from` на `d`) в круг: время в .x (меньше нуля — не входит), нормаль в .yz.
float3 enter_circle(float2 c, float r, float2 from, float2 d)
{
    float2 f = from - c;
    float a = dot(d, d);
    float cc = dot(f, f) - r * r;
    if (cc <= 0.0 || a < 1e-20)
        return float3(-1.0, 0.0, 0.0);
    float b = dot(f, d);
    float disc = b * b - a * cc;
    if (disc < 0.0)
        return float3(-1.0, 0.0, 0.0);
    float t = (-b - sqrt(disc)) / a;
    if (t < 0.0 || t > 1.0)
        return float3(-1.0, 0.0, 0.0);
    return float3(t, safe_normalize(f + d * t));
}

// Вход пути в прямоугольник: время, нормаль и ось входа (0 — x, 1 — y) в .w.
float4 enter_box(float2 c, float2 ax, float2 h, float2 from, float2 d)
{
    float2 ay = float2(-ax.y, ax.x);
    float2 rel = from - c;
    float l0[2] = { dot(rel, ax), dot(rel, ay) };
    float dl[2] = { dot(d, ax), dot(d, ay) };
    float hh[2] = { h.x, h.y };
    if (abs(l0[0]) < hh[0] && abs(l0[1]) < hh[1])
        return float4(-1.0, 0.0, 0.0, 0.0);
    float t_in = -1e30;
    float t_out = 1e30;
    uint axis = 0;
    float side = 1.0;
    for (uint i = 0; i < 2; i++)
    {
        if (abs(dl[i]) < 1e-12)
        {
            if (abs(l0[i]) >= hh[i])
                return float4(-1.0, 0.0, 0.0, 0.0);
            continue;
        }
        float t1 = (-hh[i] - l0[i]) / dl[i];
        float t2 = (hh[i] - l0[i]) / dl[i];
        float nearT = min(t1, t2);
        float farT = max(t1, t2);
        if (nearT > t_in)
        {
            t_in = nearT;
            axis = i;
            side = -sgn(dl[i]);
        }
        t_out = min(t_out, farT);
    }
    if (t_in > t_out || t_in < 0.0 || t_in > 1.0)
        return float4(-1.0, 0.0, 0.0, 0.0);
    float2 n = axis == 0 ? ax * side : ay * side;
    return float4(t_in, n, float(axis));
}

// Внутри ли точка препятствия `o` (начало его слов): глубина в .x (не меньше нуля — нет),
// куда выталкивать в .yz.
float3 inside(uint o, float2 p)
{
    uint kind = W(o);
    if (kind == KIND_BOX)
    {
        float2 c = F2(o + 4);
        float2 ax = F2(o + 6);
        float2 h = F2(o + 8);
        float rnd = F(o + 10);
        float2 ay = float2(-ax.y, ax.x);
        float2 rel = p - c;
        float2 l = float2(dot(rel, ax), dot(rel, ay));
        float2 q = abs(l) - h;
        float2 outer = max(q, float2(0.0, 0.0));
        float dist = length(outer) + min(max(q.x, q.y), 0.0) - rnd;
        if (dist >= 0.0)
            return float3(1.0, 0.0, 0.0);
        float2 nl;
        if (q.x > 0.0 || q.y > 0.0)
        {
            float2 n = safe_normalize(outer);
            nl = float2(n.x * sgn(l.x), n.y * sgn(l.y));
        }
        else if (q.x > q.y)
            nl = float2(sgn(l.x), 0.0);
        else
            nl = float2(0.0, sgn(l.y));
        return float3(dist, ax * nl.x + ay * nl.y);
    }
    if (kind == KIND_CAPSULE)
    {
        float2 a = F2(o + 4);
        float2 b = F2(o + 6);
        float r = F(o + 8);
        if (r <= 0.0)
            return float3(1.0, 0.0, 0.0);
        float2 e = b - a;
        float ee = dot(e, e);
        float t = ee > 1e-18 ? clamp(dot(p - a, e) / ee, 0.0, 1.0) : 0.0;
        float2 off = p - (a + e * t);
        float dist = length(off);
        if (dist >= r)
            return float3(1.0, 0.0, 0.0);
        float2 n = dist > 1e-9 ? off / dist : safe_normalize(float2(-e.y, e.x));
        return float3(dist - r, n);
    }
    uint first = W(o + 2);
    uint count = W(o + 3);
    uint vs = W(6) + first * 2;
    float best = -1e30;
    float2 n_best = float2(0.0, 1.0);
    for (uint i = 0; i < count; i++)
    {
        float2 p0 = F2(vs + i * 2);
        float2 p1 = F2(vs + ((i + 1) % count) * 2);
        float2 e = p1 - p0;
        float2 n = safe_normalize(float2(e.y, -e.x));
        float dist = dot(n, p - p0);
        if (dist > best)
        {
            best = dist;
            n_best = n;
        }
    }
    if (best < 0.0)
        return float3(best, n_best);
    return float3(1.0, 0.0, 0.0);
}

// Вход пути из `from` в `to` в препятствие `o`: время в .x (меньше нуля — нет), нормаль.
float3 enter(uint o, float2 from, float2 to)
{
    float2 d = to - from;
    uint kind = W(o);
    if (kind == KIND_BOX)
    {
        float rnd = F(o + 10);
        float2 h = F2(o + 8) + float2(rnd, rnd);
        float4 hit = enter_box(F2(o + 4), F2(o + 6), h, from, d);
        return hit.xyz;
    }
    if (kind == KIND_CAPSULE)
    {
        float2 a = F2(o + 4);
        float2 b = F2(o + 6);
        float r = F(o + 8);
        float2 e = b - a;
        float len = length(e);
        if (r <= 0.0)
        {
            // Тонкая линия: где путь её пересекает.
            float den = cross2(d, e);
            if (abs(den) < 1e-12)
                return float3(-1.0, 0.0, 0.0);
            float2 af = a - from;
            float t = cross2(af, e) / den;
            float u = cross2(af, d) / den;
            if (t < 0.0 || t > 1.0 || u < 0.0 || u > 1.0)
                return float3(-1.0, 0.0, 0.0);
            float2 n = safe_normalize(float2(-e.y, e.x));
            float side = dot(n, from - a);
            if (side < 0.0 || (side == 0.0 && dot(n, d) > 0.0))
                n = -n;
            return float3(t, n);
        }
        if (inside(o, from).x < 0.0)
            return float3(-1.0, 0.0, 0.0);
        float3 best = enter_circle(a, r, from, d);
        if (len > 1e-9)
        {
            float3 hb = enter_circle(b, r, from, d);
            if (hb.x >= 0.0 && (best.x < 0.0 || hb.x < best.x))
                best = hb;
            float4 hs = enter_box((a + b) * 0.5, e / len, float2(len * 0.5, r), from, d);
            if (hs.x >= 0.0 && hs.w > 0.5 && (best.x < 0.0 || hs.x < best.x))
                best = hs.xyz;
        }
        return best;
    }
    uint first = W(o + 2);
    uint count = W(o + 3);
    uint vs = W(6) + first * 2;
    float t_in = -1e30;
    float t_out = 1e30;
    float2 n_in = float2(0.0, 1.0);
    bool outside = false;
    for (uint i = 0; i < count; i++)
    {
        float2 p0 = F2(vs + i * 2);
        float2 p1 = F2(vs + ((i + 1) % count) * 2);
        float2 e = p1 - p0;
        float2 n = safe_normalize(float2(e.y, -e.x));
        float num = dot(n, from - p0);
        float den = dot(n, d);
        if (num > 0.0)
            outside = true;
        if (abs(den) < 1e-12)
        {
            if (num > 0.0)
                return float3(-1.0, 0.0, 0.0);
            continue;
        }
        float t = -num / den;
        if (den < 0.0)
        {
            if (t > t_in)
            {
                t_in = t;
                n_in = n;
            }
        }
        else
            t_out = min(t_out, t);
    }
    if (!outside || t_in > t_out || t_in < 0.0 || t_in > 1.0)
        return float3(-1.0, 0.0, 0.0);
    return float3(t_in, n_in);
}

// Скорость точки препятствия.
float2 velocity_at(uint o, float2 p)
{
    float2 r = p - F2(o + 14);
    return F2(o + 11) + float2(-r.y, r.x) * F(o + 13);
}

// Отскок от поверхности с нормалью `n`, движущейся со скоростью `pv`.
float2 respond(float2 vel, float2 n, float2 pv, float bounce)
{
    float2 rel = vel - pv;
    float vn = dot(rel, n);
    float2 vt = rel - n * vn;
    if (vn < 0.0)
        vn = -vn * bounce;
    return vt * 0.8 + n * vn + pv;
}

// Частица летит из `from` в `to`: сначала первое препятствие на пути, потом — не
// оказалась ли внутри движущегося. Как `collide` в `particle_world.rs`.
void collide(float2 from, inout float2 to, inout float2 vel, float bounce)
{
    if (W(9) == 0)
        return;
    uint cf = cell_index(from);
    uint ct = cell_index(to);
    float best_t = 2.0;
    float2 best_n = float2(0.0, 1.0);
    uint best_o = 0;
    for (uint k2 = 0; k2 < 2; k2++)
    {
        uint c = k2 == 0 ? cf : ct;
        if (c == 0xffffffffu || (k2 == 1 && c == cf))
            continue;
        uint at = W(7) + c * 2;
        uint list = W(8) + W(at);
        uint n = W(at + 1);
        for (uint j = 0; j < n; j++)
        {
            uint o = W(5) + W(list + j) * OBSTACLE_WORDS;
            float3 hit = enter(o, from, to);
            if (hit.x < 0.0)
                continue;
            if ((W(o + 1) & FLAG_ONEWAY) != 0 && hit.z <= 0.5)
                continue;
            if (hit.x < best_t)
            {
                best_t = hit.x;
                best_n = hit.yz;
                best_o = o;
            }
        }
    }
    if (best_t <= 1.0)
    {
        // Встаёт в точку касания и проезжает остаток пути вдоль поверхности.
        float2 d = to - from;
        float2 hitp = from + d * best_t + best_n * EPS;
        float2 rest = d * (1.0 - best_t);
        to = hitp + (rest - best_n * dot(rest, best_n));
        vel = respond(vel, best_n, velocity_at(best_o, hitp), bounce);
        ct = cell_index(to);
    }
    if (ct == 0xffffffffu)
        return;
    uint at = W(7) + ct * 2;
    uint list = W(8) + W(at);
    uint n = W(at + 1);
    for (uint j = 0; j < n; j++)
    {
        uint o = W(5) + W(list + j) * OBSTACLE_WORDS;
        if ((W(o + 1) & FLAG_ONEWAY) != 0)
            continue;
        float3 ins = inside(o, to);
        if (ins.x < 0.0)
        {
            to += ins.yz * (-ins.x + EPS);
            vel = respond(vel, ins.yz, velocity_at(o, to), bounce);
        }
    }
}

// Одна частица на шаг `dt`: тяжесть, воздух, движение, отскок от препятствий.
Particle step_particle(Particle p, float dt)
{
    if (p.life <= 0.0)
        return p;
    p.life -= dt;
    p.vel.y -= pc.gravity * p.gravity * dt;
    p.vel *= max(1.0 - p.drag * dt, 0.0);
    float2 to = p.pos + p.vel * dt;
    if ((p.flags & PARTICLE_COLLIDE) != 0 && pc.world_on != 0)
        collide(p.pos, to, p.vel, p.bounce);
    p.pos = to;
    return p;
}

[shader("compute")]
[numthreads(64, 1, 1)]
void cs_main(uint3 id : SV_DispatchThreadID)
{
    uint k = id.x;
    if (k >= pc.count)
        return;
    if (pc.build_list != 0 && k == 0)
    {
        // Вершин на частицу; число частиц набирается ниже (обнулено перед шагом).
        pc.draw[0] = 6;
        pc.draw[4] = 6;
    }
    uint slot = (pc.start + k) % pc.capacity;
    uint fresh = (slot + pc.capacity - pc.head) % pc.capacity;
    Particle p;
    if (fresh < pc.emit_count)
        p = pc.emitted[fresh];
    else
        p = step_particle(pc.particles[slot], pc.dt);
    pc.particles[slot] = p;
    if (pc.build_list != 0 && p.life > 0.0)
    {
        bool glow = (p.flags & PARTICLE_GLOW) != 0;
        uint index;
        InterlockedAdd(pc.draw[glow ? 5 : 1], 1u, index);
        pc.draw[LIST_NORMAL + (glow ? pc.capacity : 0) + index] = slot;
    }
}

struct VertexOut
{
    float4 position : SV_Position;
    float2 local : LOCAL;
    float4 color : COLOR;
};

static const float2 CORNERS[6] = {
    float2(-0.5, -0.5), float2(0.5, -0.5), float2(0.5, 0.5),
    float2(-0.5, -0.5), float2(0.5, 0.5), float2(-0.5, 0.5)
};

[shader("vertex")]
VertexOut vs_main(uint vertex : SV_VulkanVertexID, uint instance : SV_VulkanInstanceID)
{
    uint slot = pc.draw[LIST_NORMAL + (pc.glow_pass != 0 ? pc.capacity : 0) + instance];
    Particle p = pc.particles[slot];
    VertexOut o;
    bool glow = (p.flags & PARTICLE_GLOW) != 0;
    if (p.life <= 0.0 || glow != (pc.glow_pass != 0))
    {
        o.position = float4(2.0, 2.0, 0.0, 1.0);
        o.local = float2(0.0, 0.0);
        o.color = float4(0.0, 0.0, 0.0, 0.0);
        return o;
    }
    float t = saturate(p.life / max(p.max_life, 1e-6));
    float size = p.size * ((p.flags & PARTICLE_SHRINK) != 0 ? t : 1.0);
    // Не меньше пикселя: иначе мелкая пыль мерцает.
    size = max(size, 1.5 / (pc.zoom * 540.0));
    float2 corner = CORNERS[vertex];
    float2 s = (p.pos + corner * size - pc.center) * pc.zoom;
    o.position = float4(s.x / pc.aspect, -s.y, 0.0, 1.0);
    o.local = corner * 2.0;
    // Под конец жизни гаснет.
    o.color = float4(p.color.rgb, p.color.a * smoothstep(0.0, 0.3, t));
    return o;
}

[shader("fragment")]
float4 fs_main(VertexOut input) : SV_Target
{
    float r = length(input.local);
    // Без discard: в SPIR-V 1.6 он требует shaderDemoteToHelperInvocation, а при
    // смешивании прозрачный угол квадрата и так ничего не меняет.
    float a = input.color.a * saturate((1.0 - r) * 3.0);
    float3 c = input.color.rgb;
    // При выводе кадр уже сведён к экрану: яркость выше белого сводится к белому,
    // оттенок сохраняется (искра 3 : 0,9 : 0,15 остаётся оранжевой).
    if (pc.glow_pass != 0)
        c /= max(1.0, max(c.r, max(c.g, c.b)));
    return float4(c, a);
}

// Частицы на видеокарте (6.6): пыль, искры, брызги.
//
// Частицы лежат в кольцевом буфере. Каждый кадр вычислительный шейдер `cs_main` проходит
// по живым частицам: новые (их процессор пишет в буфер загрузки) встают на свои места,
// остальные летят, падают, тормозят о воздух и отскакивают от твёрдого уровня — тех же
// прямоугольников и склонов, на которых стоит герой. Потом пара `vs_main`/`fs_main`
// рисует каждую частицу мягким кружком. Зеркало расчёта на процессоре:
// `particles.rs`, `step_particle` (тесты сверяют поведение).

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
    uint flags;      // PARTICLE_COLLIDE, PARTICLE_SHRINK
    uint pad;
};

// Твёрдое уровня: прямоугольник, у склона ещё и подъём (как Collider в reyvec-game).
struct Solid
{
    float2 lo;
    float2 hi;
    float slope;     // подъём слева направо (0 — прямоугольник)
    uint oneway;     // платформа: держит только сверху
    uint pad0;
    uint pad1;
};

static const uint PARTICLE_COLLIDE = 1;
static const uint PARTICLE_SHRINK = 2;

struct Push
{
    Particle* particles;
    Particle* emitted;   // новые частицы этого кадра
    Solid* solids;
    uint capacity;       // мест в кольце
    uint start;          // первое место живого отрезка
    uint count;          // сколько мест от start (живые и новые)
    uint head;           // куда встаёт первая новая частица
    uint emit_count;
    uint solid_count;
    float dt;
    float gravity;       // тяжесть сцены, единиц в секунду за секунду
    // Для рисования: камера.
    float2 center;
    float aspect;
    float zoom;
};
[[vk::push_constant]] ConstantBuffer<Push> pc;

static const float EPS = 1e-4;

// Верх склона над x (как Collider::surface).
float surface(Solid s, float x)
{
    float k = saturate((x - s.lo.x) / max(s.hi.x - s.lo.x, 1e-6));
    if (s.slope < 0.0)
        k = 1.0 - k;
    return s.hi.y - abs(s.slope) * (1.0 - k);
}

// Одна частица на шаг `dt`: тяжесть, воздух, движение, отскок от твёрдого.
Particle step_particle(Particle p, float dt)
{
    if (p.life <= 0.0)
        return p;
    p.life -= dt;
    p.vel.y -= pc.gravity * p.gravity * dt;
    p.vel *= max(1.0 - p.drag * dt, 0.0);
    float2 from = p.pos;
    float2 to = p.pos + p.vel * dt;
    if ((p.flags & PARTICLE_COLLIDE) != 0)
    {
        for (uint i = 0; i < pc.solid_count; i++)
        {
            Solid s = pc.solids[i];
            if (to.x <= s.lo.x || to.x >= s.hi.x || to.y <= s.lo.y || to.y >= s.hi.y)
                continue;
            if (s.slope != 0.0)
            {
                // Склон держит сверху: была над поверхностью — встаёт на неё.
                float top = surface(s, to.x);
                if (to.y >= top || from.y < surface(s, from.x) - EPS)
                    continue;
                float2 n = normalize(float2(s.slope > 0.0 ? -abs(s.slope) : abs(s.slope),
                                            s.hi.x - s.lo.x));
                to.y = top + EPS;
                float vn = dot(p.vel, n);
                if (vn < 0.0)
                    p.vel -= (1.0 + p.bounce) * vn * n;
                p.vel *= 0.8;
                continue;
            }
            if (s.oneway != 0)
            {
                // Платформа: только упавшая сверху.
                if (from.y < s.hi.y - EPS || p.vel.y > 0.0)
                    continue;
                to.y = s.hi.y + EPS;
                p.vel.y = -p.vel.y * p.bounce;
                p.vel.x *= 0.8;
                continue;
            }
            // Откуда вошла: та сторона, которую пересекла раньше всех.
            if (from.y >= s.hi.y - EPS)
            {
                to.y = s.hi.y + EPS;
                p.vel.y = -p.vel.y * p.bounce;
                p.vel.x *= 0.8;
            }
            else if (from.y <= s.lo.y + EPS)
            {
                to.y = s.lo.y - EPS;
                p.vel.y = -p.vel.y * p.bounce;
            }
            else if (from.x <= s.lo.x + EPS)
            {
                to.x = s.lo.x - EPS;
                p.vel.x = -p.vel.x * p.bounce;
            }
            else if (from.x >= s.hi.x - EPS)
            {
                to.x = s.hi.x + EPS;
                p.vel.x = -p.vel.x * p.bounce;
            }
            else
            {
                // Появилась внутри: гаснет.
                p.life = 0.0;
            }
        }
    }
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
    uint slot = (pc.start + k) % pc.capacity;
    uint fresh = (slot + pc.capacity - pc.head) % pc.capacity;
    if (fresh < pc.emit_count)
    {
        pc.particles[slot] = pc.emitted[fresh];
        return;
    }
    pc.particles[slot] = step_particle(pc.particles[slot], pc.dt);
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
    Particle p = pc.particles[(pc.start + instance) % pc.capacity];
    VertexOut o;
    if (p.life <= 0.0)
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
    float a = input.color.a * saturate((1.0 - r) * 3.0);
    if (a <= 0.0)
        discard;
    return float4(input.color.rgb, a);
}

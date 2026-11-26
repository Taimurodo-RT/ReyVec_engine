# Generates world.ron: the "big world" scene of stage 8 (tasks 8.9-8.11), a town 400 m
# across seen from a hill. Own work, CC0. Run: python3 world.py (writes world.ron next to it).
#
# 15 x 15 blocks, 26 m apart. A block: a house (walls, roof, glowing windows, chimney),
# a fence, a tree, a street lamp post. Around the town: far towers and a ring of hills.
# Moving things: cars on the streets, windmills, a balloon.
import math, os

BLOCKS = 15
STEP = 26.0
HALF = (BLOCKS - 1) * STEP / 2

objects = []


def f(v):
    return f"{v:.2f}"


def vec(v):
    return "(" + ", ".join(f(c) for c in v) + ")"


def obj(name, material, at, shape, extra=""):
    objects.append(f'        (name: "{name}", material: "{material}", at: {vec(at)}{extra}, shape: {shape}),')


def box(size, round_=0.0):
    r = f", round: {f(round_)}" if round_ else ""
    return f"Box(size: {vec(size)}{r})"


def move(at, shape, turn=None):
    t = f"turn: {vec(turn)}, " if turn else ""
    return f"Move(at: {vec(at)}, {t}shape: {shape})"


def union(parts):
    return "Union(shapes: [" + ", ".join(parts) + "])"


def intersect(parts):
    return "Intersect(shapes: [" + ", ".join(parts) + "])"


def rnd(i, j, k):
    """Repeatable number 0..1 for block (i, j), stream k."""
    x = math.sin(i * 127.1 + j * 311.7 + k * 74.7) * 43758.5453
    return x - math.floor(x)


WALLS = ("стена белая", "стена жёлтая", "стена кирпич", "стена серая")
ROOFS = ("крыша красная", "крыша тёмная", "крыша зелёная")

materials = """        (name: "земля", color: (0.30, 0.36, 0.24), grid: true),
        (name: "дорога", color: (0.16, 0.16, 0.17)),
        (name: "стена белая", color: (0.82, 0.80, 0.74)),
        (name: "стена жёлтая", color: (0.85, 0.70, 0.40)),
        (name: "стена кирпич", color: (0.62, 0.30, 0.22)),
        (name: "стена серая", color: (0.52, 0.54, 0.57)),
        (name: "крыша красная", color: (0.60, 0.16, 0.12)),
        (name: "крыша тёмная", color: (0.20, 0.21, 0.24)),
        (name: "крыша зелёная", color: (0.18, 0.38, 0.26)),
        (name: "окно", color: (1.0, 0.85, 0.55), glow: 1.5),
        (name: "дерево", color: (0.36, 0.25, 0.16)),
        (name: "листва", color: (0.20, 0.48, 0.20)),
        (name: "столб", color: (0.25, 0.26, 0.28)),
        (name: "фонарь", color: (1.0, 0.90, 0.70), glow: 4.0),
        (name: "камень", color: (0.58, 0.57, 0.54)),
        (name: "холм", color: (0.26, 0.40, 0.24)),
        (name: "машина красная", color: (0.80, 0.14, 0.10), shine: 0.3, rough: 0.2),
        (name: "машина синяя", color: (0.12, 0.30, 0.75), shine: 0.3, rough: 0.2),
        (name: "шар", color: (0.95, 0.45, 0.10)),"""

# Streets: dark strips just above the ground, between the blocks.
for i in range(BLOCKS + 1):
    c = -HALF - STEP / 2 + i * STEP
    obj(f"улица запад-восток {i + 1}", "дорога", (0.0, 0.01, c), box((2 * HALF + STEP + 6.0, 0.02, 6.0)))
    obj(f"улица север-юг {i + 1}", "дорога", (c, 0.012, 0.0), box((6.0, 0.02, 2 * HALF + STEP + 6.0)))

for i in range(BLOCKS):
    for j in range(BLOCKS):
        x = -HALF + i * STEP
        z = -HALF + j * STEP
        tag = f"{i + 1}-{j + 1}"
        # The middle block is a square with a tower.
        if i == BLOCKS // 2 and j == BLOCKS // 2:
            obj("башня", "камень", (x, 11.0, z), union([
                box((5.0, 22.0, 5.0), 0.1),
                move((0.0, 12.5, 0.0), box((6.0, 3.0, 6.0), 0.1)),
                move((0.0, 16.0, 0.0), "Cone(height: 4.0, bottom: 3.6, top: 0.0)"),
            ]))
            obj("часы башни", "окно", (x, 18.0, z + 2.56), "Cylinder(height: 0.1, radius: 1.2, round: 0.0)",
                ", turn: (90.0, 0.0, 0.0)")
            continue
        w = 7.0 + 5.0 * rnd(i, j, 1)
        d = 6.0 + 4.0 * rnd(i, j, 2)
        floors = 1 + int(3.0 * rnd(i, j, 3))
        h = 3.0 * floors
        hx = x + (rnd(i, j, 4) - 0.5) * 4.0
        hz = z + (rnd(i, j, 5) - 0.5) * 3.0
        wall = WALLS[int(rnd(i, j, 6) * len(WALLS))]
        roof = ROOFS[int(rnd(i, j, 7) * len(ROOFS))]
        obj(f"дом {tag}", wall, (hx, h / 2, hz), union([
            box((w, h, d)),
            # Chimney: a thin thing, hard for far views.
            move((w * 0.3, h / 2 + 1.6, d * 0.2), box((0.5, 1.6, 0.5))),
            # Porch.
            move((0.0, -h / 2 + 0.15, d / 2 + 0.6), box((2.0, 0.3, 1.2))),
        ]))
        # Roof: a flat slab with a gable on it. The gable is the upper half of a box turned
        # 45 degrees; its ends are flush with the slab (a 10 cm lip there is thinner than
        # a pixel from the street and draws as dashes).
        obj(f"крыша {tag}", roof, (hx, h + 0.15, hz), union([
            box((w + 0.6, 0.3, d + 0.6)),
            intersect([
                move((0.0, 0.15, 0.0), box((w + 0.6, d * 0.5, d * 0.5)), (45.0, 0.0, 0.0)),
                move((0.0, d * 0.25, 0.0), box((w + 0.6, d * 0.5, d))),
            ]),
        ]))
        # Windows: a row per floor on the south and north walls, one glowing object.
        panes = []
        n = max(2, int(w / 2.2))
        for fl in range(floors):
            for k in range(n):
                wx = -w / 2 + (k + 0.5) * w / n
                wy = -h / 2 + 1.7 + fl * 3.0
                if fl == 0 and abs(wx) < 1.0:
                    continue
                panes.append(move((wx, wy, d / 2), box((0.9, 1.2, 0.08))))
                panes.append(move((wx, wy, -d / 2), box((0.9, 1.2, 0.08))))
        obj(f"окна {tag}", "окно", (hx, h / 2, hz), union(panes))
        # Fence along the south edge of the block: posts and two rails.
        fz = z + STEP / 2 - 4.0
        rails = [move((0.0, 0.25, 0.0), box((16.0, 0.08, 0.05))), move((0.0, -0.15, 0.0), box((16.0, 0.08, 0.05)))]
        posts = [move((-8.0 + p * 2.0, 0.0, 0.0), box((0.1, 1.0, 0.1))) for p in range(9)]
        obj(f"забор {tag}", "дерево", (x, 0.5, fz), union(rails + posts))
        # Tree.
        tx = x - STEP / 2 + 5.0 + rnd(i, j, 8) * 3.0
        tz = z - STEP / 2 + 5.0 + rnd(i, j, 9) * 3.0
        th = 2.0 + 2.0 * rnd(i, j, 10)
        obj(f"ствол {tag}", "дерево", (tx, th / 2, tz), f"Cylinder(height: {f(th)}, radius: 0.18, round: 0.0)")
        obj(f"крона {tag}", "листва", (tx, th + 1.2, tz), union([
            f"Sphere(radius: {f(1.4 + rnd(i, j, 11) * 0.6)})",
            move((0.7, 0.6, 0.3), "Sphere(radius: 1.0)"),
            move((-0.6, 0.4, -0.5), "Sphere(radius: 1.1)"),
        ]))
        # Street lamp post at the block corner: post, arm and a glowing head.
        lx = x + STEP / 2 - 3.6
        lz = z + STEP / 2 - 3.6
        obj(f"столб {tag}", "столб", (lx, 2.5, lz), union([
            "Cylinder(height: 5.0, radius: 0.07, round: 0.0)",
            move((0.4, 2.45, 0.0), box((0.9, 0.06, 0.06))),
        ]))
        obj(f"фонарь {tag}", "фонарь", (lx + 0.8, 4.85, lz), "Sphere(radius: 0.16)")

# Far things: towers around the town and a ring of hills behind them.
for k in range(12):
    a = k / 12 * math.tau
    r = HALF + 60.0 + 20.0 * rnd(k, 0, 20)
    h = 25.0 + 30.0 * rnd(k, 0, 21)
    obj(f"вышка {k + 1}", "камень", (math.cos(a) * r, h / 2, math.sin(a) * r), union([
        box((3.0, h, 3.0)),
        move((0.0, h / 2 + 3.0, 0.0), "Cylinder(height: 6.0, radius: 0.1, round: 0.0)"),
    ]))
for k in range(16):
    a = (k + 0.5) / 16 * math.tau
    r = HALF + 130.0
    s = 50.0 + 30.0 * rnd(k, 1, 22)
    obj(f"холм {k + 1}", "холм", (math.cos(a) * r, -s * 0.55, math.sin(a) * r), f"Sphere(radius: {f(s)})")

# Moving: cars along the streets, windmills, a balloon.
for k in range(24):
    street = -HALF - STEP / 2 + (1 + k % (BLOCKS - 1)) * STEP
    along = (rnd(k, 2, 23) - 0.5) * 2 * HALF * 0.6
    paint = "машина красная" if k % 2 else "машина синяя"
    car = union([box((3.6, 0.8, 1.6), 0.15), move((-0.2, 0.7, 0.0), box((1.9, 0.7, 1.4), 0.15))])
    if k % 2:
        obj(f"машина {k + 1}", paint, (along, 0.6, street - 1.4), car,
            f", bob: (by: (40.0, 0.0, 0.0), period: {f(14.0 + k % 5)})")
    else:
        obj(f"машина {k + 1}", paint, (street + 1.4, 0.6, along), car,
            f", turn: (0.0, 90.0, 0.0), bob: (by: (0.0, 0.0, 40.0), period: {f(15.0 + k % 4)})")
for k in range(4):
    a = k / 4 * math.tau + 0.4
    r = HALF + 30.0
    x, z = math.cos(a) * r, math.sin(a) * r
    obj(f"мельница {k + 1}", "стена белая", (x, 6.0, z), "Cone(height: 12.0, bottom: 2.5, top: 1.2)")
    obj(f"крылья {k + 1}", "дерево", (x, 10.5, z + 2.0), union([box((14.0, 0.5, 0.1)), box((0.5, 14.0, 0.1))]),
        f", spin: (0.0, 0.0, {f(25.0 + k * 6.0)})")
obj("воздушный шар", "шар", (0.0, 45.0, -40.0), union([
    "Sphere(radius: 5.0)",
    move((0.0, -7.5, 0.0), box((1.6, 1.2, 1.6), 0.1)),
]), ", circle: (radius: (60.0, 60.0), period: 90.0)")

count = len(objects) + 1  # + ground
size = int(2 * (HALF + 130.0 + 80.0))
header = f"""// Сцена «большой мир» этапа 8 (задачи 8.9–8.11): город {int(2 * HALF + STEP)} м в поперечнике, с вышками
// и холмами — около {size} м. {count} предметов. Запуск:
//     .\\run.bat --space-file scenes\\3d\\world.ron
// потом клавиша I. Файл собран скриптом world.py (рядом): {BLOCKS} × {BLOCKS} кварталов.
// В квартале — дом (стены, крыша, светящиеся окна, труба), забор, дерево, фонарный столб.
// Посередине площадь с башней. Вокруг вышки с антеннами и холмы. Движутся машины,
// крылья мельниц и воздушный шар.
// Что смотреть: вид с холма на весь город — далёкие заборы, столбы и окна не должны
// мерцать при движении камеры; время кадра не должно расти от того, что город далеко.
// Править лучше скрипт.
(
    format: "reyvec-space",
    version: 1,
    camera: Some((eye: (-30.0, 38.0, {f(HALF + 75.0)}), look_at: (0.0, 2.0, 0.0))),
    // Дымка: даль уходит в небо не на 63 м, как в комнатах, а на 700.
    haze: 700.0,

    materials: [
{materials}
    ],

    objects: [
        (name: "земля", material: "земля", shape: Plane(normal: (0.0, 1.0, 0.0), offset: 0.0)),
"""
text = header + "\n".join(objects) + "\n    ],\n)\n"
here = os.path.dirname(os.path.abspath(__file__))
with open(os.path.join(here, "world.ron"), "w", encoding="utf-8") as out:
    out.write(text)
print(count, "objects")

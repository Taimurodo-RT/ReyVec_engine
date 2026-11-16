# Generates factory.ron: the 1000+ object scene of stage 8 (task 8.7), a factory hall
# seen from above. Own work, CC0. Run: python3 factory.py (writes factory.ron next to it).
#
# 8 production lines x 12 stations. A station: machine model (models/machine.glb, its
# shadow is one shared field brick), conveyor with legs, two crates riding the belt,
# control cabinet, barrel, spinning fan, overhead pipe. Around: columns, walls, pallet
# racks with boxes, forklifts driving in circles, lamp shades.
import math, os

LINES, STATIONS = 8, 12
STEP_X, STEP_Z = 4.0, 5.0
X0 = -(STATIONS - 1) * STEP_X / 2
Z0 = -(LINES - 1) * STEP_Z / 2
HALL_X, HALL_Z = 30.0, 26.0  # half sizes

objects, meshes = [], []


def f(v):
    return f"{v:.2f}"


def vec(v):
    return "(" + ", ".join(f(c) for c in v) + ")"


def obj(name, material, at, shape, extra=""):
    objects.append(f'        (name: "{name}", material: "{material}", at: {vec(at)}{extra}, shape: {shape}),')


def box(size, round_=0.0):
    r = f", round: {f(round_)}" if round_ else ""
    return f"Box(size: {vec(size)}{r})"


def move(at, shape):
    return f"Move(at: {vec(at)}, shape: {shape})"


def union(parts):
    return "Union(shapes: [" + ", ".join(parts) + "])"


materials = """        (name: "пол", color: (0.20, 0.21, 0.22), grid: true),
        (name: "лента", color: (0.10, 0.10, 0.11)),
        (name: "сталь", color: (0.55, 0.57, 0.60)),
        (name: "ящик", color: (0.72, 0.52, 0.30)),
        (name: "шкаф", color: (0.25, 0.45, 0.70)),
        (name: "бочка", color: (0.75, 0.18, 0.12)),
        (name: "жёлтый", color: (0.92, 0.72, 0.12)),
        (name: "бетон", color: (0.62, 0.61, 0.58)),
        (name: "труба", color: (0.35, 0.55, 0.40)),
        (name: "плафон", color: (0.95, 0.90, 0.75), glow: 2.0),"""

for line in range(LINES):
    z = Z0 + line * STEP_Z
    for st in range(STATIONS):
        x = X0 + st * STEP_X
        k = line * STATIONS + st
        tag = f"{line + 1}-{st + 1}"
        meshes.append(
            f'        (name: "станок {tag}", at: {vec((x - 0.9, 0.0, z - 1.3))}, turn: (0.0, 0.0, 0.0), '
            f'mesh: Model(file: "models/machine.glb")),'
        )
        # Conveyor: belt and four legs, one object.
        legs = [move((dx, -0.33, dz), box((0.08, 0.6, 0.08))) for dx in (-1.6, 1.6) for dz in (-0.3, 0.3)]
        obj(f"конвейер {tag}", "лента", (x, 0.66, z), union([box((3.6, 0.12, 0.8), 0.02)] + legs))
        # Two crates riding the belt back and forth (out of phase).
        for c, shift in enumerate((-0.8, 0.8)):
            period = 4.0 + (k % 5) * 0.37
            obj(
                f"ящик {tag}.{c + 1}", "ящик", (x + shift, 0.92, z),
                box((0.4, 0.4, 0.4), 0.02),
                f", bob: (by: ({f(-shift)}, 0.0, 0.0), period: {f(period)})",
            )
        obj(f"шкаф {tag}", "шкаф", (x + 1.2, 0.6, z - 1.4), box((0.5, 1.2, 0.4), 0.03))
        obj(f"бочка {tag}", "бочка", (x + 1.4, 0.45, z + 1.2), "Cylinder(height: 0.9, radius: 0.28, round: 0.03)")
        obj(
            f"вентилятор {tag}", "сталь", (x - 1.1, 1.75, z - 1.3),
            union([box((0.9, 0.04, 0.12)), box((0.12, 0.04, 0.9))]),
            f", spin: (0.0, {f(120.0 + (k % 7) * 25.0)}, 0.0)",
        )
        obj(
            f"труба {tag}", "труба", (x, 3.2, z + 0.9),
            "Move(turn: (0.0, 0.0, 90.0), shape: Capsule(height: 3.6, radius: 0.12))",
        )

# Columns.
for i in range(9):
    for j in range(5):
        x = -24.0 + i * 6.0
        z = -20.0 + j * 10.0
        obj(f"колонна {i + 1}-{j + 1}", "бетон", (x, 2.5, z - 2.5), box((0.5, 5.0, 0.5)))

# Walls (low, so the camera sees in).
for name, at, size in (
    ("стена север", (0.0, 0.75, -HALL_Z), (2 * HALL_X, 1.5, 0.3)),
    ("стена юг", (0.0, 0.75, HALL_Z), (2 * HALL_X, 1.5, 0.3)),
    ("стена запад", (-HALL_X, 0.75, 0.0), (0.3, 1.5, 2 * HALL_Z)),
    ("стена восток", (HALL_X, 0.75, 0.0), (0.3, 1.5, 2 * HALL_Z)),
):
    obj(name, "бетон", at, box(size))

# Pallet racks along the north and south walls: shelves with boxes.
for side, z in (("север", -HALL_Z + 1.2), ("юг", HALL_Z - 1.2)):
    for i in range(26):
        x = -27.0 + i * 2.16
        obj(f"стеллаж {side} {i + 1}", "сталь", (x, 1.0, z), union(
            [move((dx, 0.0, 0.0), box((0.06, 2.0, 0.8))) for dx in (-0.95, 0.95)]
            + [move((0.0, dy, 0.0), box((1.96, 0.05, 0.8))) for dy in (-0.55, 0.15)]
        ))
        for lvl, y in enumerate((0.75, 1.45)):
            for b in range(2):
                dx = -0.45 + b * 0.9
                s = 0.45 + ((i * 3 + lvl * 5 + b * 7) % 4) * 0.05
                obj(f"коробка {side} {i + 1}.{lvl * 2 + b + 1}", "ящик", (x + dx, y - 0.25 + s / 2, z), box((0.7, s, 0.6), 0.02))

# Forklifts driving in ovals in the aisles.
for i in range(20):
    x = -24.0 + (i % 10) * 5.3
    z = -22.0 if i < 10 else 22.0
    body = union([
        box((1.0, 0.5, 0.7), 0.05),
        move((-0.15, 0.55, 0.0), box((0.5, 0.6, 0.6), 0.03)),
        move((0.6, 0.6, 0.0), box((0.06, 1.2, 0.5))),
    ])
    obj(
        f"погрузчик {i + 1}", "жёлтый", (x, 0.35, z), body,
        f", circle: (radius: (1.4, 0.6), period: {f(9.0 + (i % 4) * 1.5)})",
    )

# Lamp shades over the lines.
for i in range(4):
    for j in range(2):
        obj(f"плафон {i * 2 + j + 1}", "плафон", (-15.0 + i * 10.0, 4.6, -6.0 + j * 12.0),
            "Cylinder(height: 0.12, radius: 0.5, round: 0.03)")

lamps = [
    "        (at: (-6.0, 4.0, -2.5), color: (1.0, 0.85, 0.65), power: 14.0, range: 14.0,",
    "            circle: (radius: (6.0, 2.0), period: 20.0)),",
    "        (at: (10.0, 4.0, 5.0), color: (0.65, 0.80, 1.0), power: 12.0, range: 12.0),",
    "        (at: (-20.0, 4.0, 12.0), color: (1.0, 0.9, 0.75), power: 10.0, range: 12.0),",
    "        (at: (20.0, 4.0, -15.0), color: (1.0, 0.9, 0.75), power: 10.0, range: 12.0),",
]

count = len(objects) + 1  # + floor
header = f"""// Сцена-нагрузка этапа 8 (задача 8.7): цех завода, {count} предметов и {len(meshes)} станков-моделей.
// Запуск:
//     .\\run.bat --space-file scenes\\3d\\factory.ron
// потом клавиша I. Файл собран скриптом factory.py (рядом): {LINES} линий по {STATIONS} станков.
// У станка — модель из models/machine.glb (тень от общего «кирпича поля»), конвейер на
// ножках, два ящика едут по ленте, шкаф, бочка, вентилятор крутится, труба над линией.
// Вокруг колонны, стены, стеллажи с коробками, погрузчики ездят по кругу, плафоны.
// Это точка отсчёта для ускорения масштаба (8.8, 8.9): на панели видно, сколько предметов
// в кадре и сколько стоит подготовка кадра на процессоре. Править лучше скрипт.
(
    format: "reyvec-space",
    version: 1,
    camera: Some((eye: (0.0, 22.0, 30.0), look_at: (0.0, 0.0, 2.0))),

    materials: [
{materials}
    ],

    lamps: [
{chr(10).join(lamps)}
    ],

    objects: [
        (name: "пол", material: "пол", shape: Plane(normal: (0.0, 1.0, 0.0), offset: 0.0)),
"""
text = header + "\n".join(objects) + "\n    ],\n\n    meshes: [\n" + "\n".join(meshes) + "\n    ],\n)\n"
with open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "factory.ron"), "w", encoding="utf-8") as out:
    out.write(text)
print(count, "objects,", len(meshes), "meshes")

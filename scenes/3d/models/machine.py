# Generates machine.glb: a simple factory machine for the glTF loading test (task 8.14).
# Own work, CC0. Run: python3 machine.py (writes machine.glb next to this file).
import json, math, struct, os

def box(w, h, d):
    x, y, z = w / 2, h / 2, d / 2
    p = [(-x,-y,-z),(x,-y,-z),(x,y,-z),(-x,y,-z),(-x,-y,z),(x,-y,z),(x,y,z),(-x,y,z)]
    i = [0,2,1, 0,3,2, 4,5,6, 4,6,7, 0,1,5, 0,5,4, 3,7,6, 3,6,2, 0,4,7, 0,7,3, 1,2,6, 1,6,5]
    return p, i

def cylinder(r, h, sides):
    # Side with radial normals (smooth), caps with their own vertices and flat normals.
    p, n, i = [], [], []
    for k in range(sides):
        a = 2 * math.pi * k / sides
        c, s_ = math.cos(a), -math.sin(a)
        p += [(r * c, -h / 2, r * s_), (r * c, h / 2, r * s_)]
        n += [(c, 0, s_), (c, 0, s_)]
    for k in range(sides):
        a0, a1 = 2 * k, 2 * k + 1
        b0, b1 = 2 * ((k + 1) % sides), 2 * ((k + 1) % sides) + 1
        i += [a0, b0, b1, a0, b1, a1]
    for y, ny in ((-h / 2, -1), (h / 2, 1)):
        base = len(p)
        for k in range(sides):
            a = 2 * math.pi * k / sides
            p.append((r * math.cos(a), y, -r * math.sin(a)))
            n.append((0, ny, 0))
        p.append((0, y, 0))
        n.append((0, ny, 0))
        center = base + sides
        for k in range(sides):
            a, b = base + k, base + (k + 1) % sides
            i += [center, b, a] if ny < 0 else [center, a, b]
    return p, i, n

blob = bytearray()
views, accessors, meshes = [], [], []

def add(data, target):
    while len(blob) % 4:
        blob.append(0)
    views.append({"buffer": 0, "byteOffset": len(blob), "byteLength": len(data), "target": target})
    blob.extend(data)
    return len(views) - 1

def mesh(name, material, p, i=None, n=None):
    pos = add(b"".join(struct.pack("<3f", *v) for v in p), 34962)
    lo = [min(v[k] for v in p) for k in range(3)]
    hi = [max(v[k] for v in p) for k in range(3)]
    accessors.append({"bufferView": pos, "componentType": 5126, "count": len(p), "type": "VEC3", "min": lo, "max": hi})
    prim = {"attributes": {"POSITION": len(accessors) - 1}, "material": material}
    if n is not None:
        nv = add(b"".join(struct.pack("<3f", *v) for v in n), 34962)
        accessors.append({"bufferView": nv, "componentType": 5126, "count": len(n), "type": "VEC3"})
        prim["attributes"]["NORMAL"] = len(accessors) - 1
    if i is not None:
        idx = add(struct.pack("<%dH" % len(i), *i), 34963)
        accessors.append({"bufferView": idx, "componentType": 5123, "count": len(i), "type": "SCALAR"})
        prim["indices"] = len(accessors) - 1
    meshes.append({"name": name, "primitives": [prim]})
    return len(meshes) - 1

def unindexed(p, i):
    return [p[k] for k in i]

materials = [
    {"name": "станина", "pbrMetallicRoughness": {"baseColorFactor": [0.08, 0.085, 0.09, 1.0]}},
    {"name": "корпус", "pbrMetallicRoughness": {"baseColorFactor": [0.80, 0.30, 0.05, 1.0]}},
    {"name": "бак", "pbrMetallicRoughness": {"baseColorFactor": [0.20, 0.32, 0.45, 1.0]}},
    {"name": "труба", "pbrMetallicRoughness": {"baseColorFactor": [0.55, 0.55, 0.52, 1.0]}},
]
m_base = mesh("станина", 0, *box(1.6, 0.2, 1.0))
m_body = mesh("корпус", 1, *box(0.9, 0.8, 0.7))
m_tank = mesh("бак", 2, *cylinder(0.22, 0.9, 16))
m_pipe = mesh("труба", 3, *cylinder(0.06, 0.62, 12))
m_chimney = mesh("дымоход", 0, unindexed(*box(0.15, 0.5, 0.15)))
s = math.sqrt(0.5)
nodes = [
    {"name": "станок", "children": [1, 2, 3, 4, 5, 6], "translation": [0.0, 0.0, 0.0]},
    {"name": "станина", "mesh": m_base, "translation": [0.0, 0.1, 0.0]},
    {"name": "корпус", "mesh": m_body, "translation": [-0.25, 0.6, 0.0]},
    {"name": "бак 1", "mesh": m_tank, "translation": [0.5, 0.65, 0.22]},
    {"name": "бак 2", "mesh": m_tank, "translation": [0.5, 0.65, -0.24], "scale": [0.9, 0.8, 0.9]},
    # Pipe lies along X: cylinder turned 90 degrees around Z.
    {"name": "труба", "mesh": m_pipe, "translation": [0.2, 0.85, 0.22], "rotation": [0.0, 0.0, s, s]},
    {"name": "дымоход", "mesh": m_chimney, "translation": [-0.5, 1.25, 0.12]},
]
doc = {
    "asset": {"version": "2.0", "generator": "ReyVec machine.py", "copyright": "CC0"},
    "scene": 0, "scenes": [{"nodes": [0]}], "nodes": nodes, "meshes": meshes,
    "materials": materials, "accessors": accessors, "bufferViews": views,
    "buffers": [{"byteLength": len(blob)}],
}
js = json.dumps(doc, ensure_ascii=False).encode()
js += b" " * (-len(js) % 4)
while len(blob) % 4:
    blob.append(0)
out = struct.pack("<3I", 0x46546C67, 2, 12 + 8 + len(js) + 8 + len(blob))
out += struct.pack("<2I", len(js), 0x4E4F534A) + js + struct.pack("<2I", len(blob), 0x004E4942) + bytes(blob)
with open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "machine.glb"), "wb") as f:
    f.write(out)

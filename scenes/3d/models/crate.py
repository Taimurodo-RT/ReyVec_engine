# Generates crate.glb: a wooden crate with a color texture (task 8.14b2): one box,
# 24 vertices with normals and texture points, the picture as PNG inside the file.
# Own work, CC0. Run: python3 crate.py (writes crate.glb next to this file).
import json, struct, os, zlib

SIZE = 64

def crate_pixel(x, y):
    # Frame of dark boards, light planks inside, a diagonal brace, nail heads.
    dark, light, brace, nail = (92, 56, 28), (176, 124, 70), (140, 92, 48), (40, 40, 44)
    border = 7
    if x < border or y < border or x >= SIZE - border or y >= SIZE - border:
        c = dark
        if (x in (3, SIZE - 4)) and (y in (3, SIZE - 4)):
            c = nail
    elif abs(x - y) <= 4:
        c = brace
    else:
        c = light
        if (y - border) % 10 == 0:
            c = dark  # gaps between planks
    # A little grain so the mip levels have something to average.
    k = ((x * 7 + y * 13) % 11) - 5
    return tuple(max(0, min(255, v + k)) for v in c)

def png(width, height, pixel):
    raw = b"".join(b"\0" + b"".join(bytes(pixel(x, y)) + b"\xff" for x in range(width)) for y in range(height))
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)
    head = struct.pack(">2I5B", width, height, 8, 6, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", head) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")

# Box 0.7 m, each face its own 4 vertices: normal and texture points 0..1.
h = 0.35
faces = [
    ((1, 0, 0), [(h, -h, h), (h, -h, -h), (h, h, -h), (h, h, h)]),
    ((-1, 0, 0), [(-h, -h, -h), (-h, -h, h), (-h, h, h), (-h, h, -h)]),
    ((0, 1, 0), [(-h, h, h), (h, h, h), (h, h, -h), (-h, h, -h)]),
    ((0, -1, 0), [(-h, -h, -h), (h, -h, -h), (h, -h, h), (-h, -h, h)]),
    ((0, 0, 1), [(-h, -h, h), (h, -h, h), (h, h, h), (-h, h, h)]),
    ((0, 0, -1), [(h, -h, -h), (-h, -h, -h), (-h, h, -h), (h, h, -h)]),
]
p, n, uv, idx = [], [], [], []
for normal, corners in faces:
    base = len(p)
    p += corners
    n += [normal] * 4
    uv += [(0.0, 1.0), (1.0, 1.0), (1.0, 0.0), (0.0, 0.0)]
    idx += [base, base + 1, base + 2, base, base + 2, base + 3]

blob = bytearray()
views, accessors = [], []

def add(data, target=None):
    while len(blob) % 4:
        blob.append(0)
    view = {"buffer": 0, "byteOffset": len(blob), "byteLength": len(data)}
    if target:
        view["target"] = target
    views.append(view)
    blob.extend(data)
    return len(views) - 1

def accessor(view, ctype, count, kind, **extra):
    accessors.append({"bufferView": view, "componentType": ctype, "count": count, "type": kind, **extra})
    return len(accessors) - 1

a_pos = accessor(add(b"".join(struct.pack("<3f", *v) for v in p), 34962), 5126, len(p), "VEC3",
                 min=[-h] * 3, max=[h] * 3)
a_nrm = accessor(add(b"".join(struct.pack("<3f", *v) for v in n), 34962), 5126, len(n), "VEC3")
a_uv = accessor(add(b"".join(struct.pack("<2f", *v) for v in uv), 34962), 5126, len(uv), "VEC2")
a_idx = accessor(add(struct.pack("<%dH" % len(idx), *idx), 34963), 5123, len(idx), "SCALAR")
image_view = add(png(SIZE, SIZE, crate_pixel))

doc = {
    "asset": {"version": "2.0", "generator": "ReyVec crate.py", "copyright": "CC0"},
    "scene": 0, "scenes": [{"nodes": [0]}],
    "nodes": [{"name": "ящик", "mesh": 0, "translation": [0.0, h, 0.0]}],
    "meshes": [{"name": "ящик", "primitives": [{
        "attributes": {"POSITION": a_pos, "NORMAL": a_nrm, "TEXCOORD_0": a_uv},
        "indices": a_idx, "material": 0}]}],
    "materials": [{"name": "доски", "pbrMetallicRoughness": {
        "baseColorFactor": [1.0, 1.0, 1.0, 1.0], "baseColorTexture": {"index": 0}}}],
    "textures": [{"source": 0, "sampler": 0}],
    "samplers": [{"magFilter": 9729, "minFilter": 9987}],
    "images": [{"name": "доски", "bufferView": image_view, "mimeType": "image/png"}],
    "accessors": accessors, "bufferViews": views,
    "buffers": [{"byteLength": len(blob)}],
}
js = json.dumps(doc, ensure_ascii=False).encode()
js += b" " * (-len(js) % 4)
while len(blob) % 4:
    blob.append(0)
out = struct.pack("<3I", 0x46546C67, 2, 12 + 8 + len(js) + 8 + len(blob))
out += struct.pack("<2I", len(js), 0x4E4F534A) + js + struct.pack("<2I", len(blob), 0x004E4942) + bytes(blob)
with open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "crate.glb"), "wb") as f:
    f.write(out)

import os
"""Render office.tmj with the (new) generated tilesets -> office-new.png.

Layers drawn: floor, walls, furniture-below, furniture-above (collision
skipped). Gids are masked with 0x1FFFFFFF; Tiled H/V/D flip flags are
applied. Tiles are 16px in the atlas, pasted at 32px.
"""
import json
from PIL import Image

MAP = os.path.join(os.path.dirname(__file__), "..", "src", "assets", "maps", "office.tmj")
TS = os.path.join(os.path.dirname(__file__), "..", "src", "assets", "tilesets")
OUT = os.path.join(os.path.dirname(__file__), "..", "office-new.png")
SCALE = 2  # 16px tiles -> 32px on screen

SHEETS = os.path.join(os.path.dirname(__file__), "..", "src", "assets", "tilesets")
    (1, Image.open(TS + r"\office-tileset.png").convert("RGBA"), 512),
    (513, Image.open(TS + r"\a5-office-floors-walls.png").convert("RGBA"), 512),
    (1025, Image.open(TS + r"\interiors.png").convert("RGBA"), 1424),
]

def tile_for(gid):
    for first, sheet, count in reversed(SHEETS):
        if gid >= first:
            tid = gid - first
            if tid >= count:
                return None
            x, y = (tid % 16) * 16, (tid // 16) * 16
            return sheet.crop((x, y, x + 16, y + 16))
    return None

def apply_flags(tile, raw):
    H, V, D = 0x80000000, 0x40000000, 0x20000000
    if raw & D:
        tile = tile.transpose(Image.TRANSPOSE)
    if raw & H:
        tile = tile.transpose(Image.FLIP_LEFT_RIGHT)
    if raw & V:
        tile = tile.transpose(Image.FLIP_TOP_BOTTOM)
    return tile

def main():
    m = json.load(open(MAP))
    W, H = m["width"], m["height"]
    pad = 16
    canvas = Image.new("RGBA", (W * 16 * SCALE + pad * 2,
                                H * 16 * SCALE + pad * 2), (30, 30, 36, 255))
    # white frame like the app bg
    from PIL import ImageDraw
    dr = ImageDraw.Draw(canvas)
    dr.rectangle([4, 4, canvas.width - 5, canvas.height - 5], fill=(240, 240, 240, 255))
    dr.rectangle([pad - 4, pad - 4, canvas.width - pad + 3, canvas.height - pad + 3],
                 fill=(30, 30, 36, 255))
    for layer in m["layers"]:
        if layer.get("type") != "tilelayer" or layer["name"] == "collision":
            continue
        for i, raw in enumerate(layer["data"]):
            if not raw:
                continue
            gid = raw & 0x1FFFFFFF
            t = tile_for(gid)
            if t is None:
                continue
            t = apply_flags(t, raw)
            t = t.resize((16 * SCALE, 16 * SCALE), Image.NEAREST)
            canvas.paste(t, (pad + (i % W) * 16 * SCALE,
                             pad + (i // W) * 16 * SCALE), t)
    canvas.save(OUT)
    print("wrote", OUT, canvas.size)

if __name__ == "__main__":
    main()

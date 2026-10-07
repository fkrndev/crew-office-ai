"""
Generate replacement tilesets for the office app.

Replaces (same dimensions / 16x16 grid / 16 cols, alpha preserved):
  - office-tileset.png        256x512   gids 1-512    (tid = gid - 1)
  - a5-office-floors-walls.png 256x512  gids 513-1024 (tid = gid - 513)
  - interiors.png             256x1424  gids 1025-2448 (tid = gid - 1025)

Furniture slots are composited from Antea's CC-BY "Office Furniture
Pixel Art" sprites (see ANTEA_DIR); floors, walls, carpet, counters,
doors and the doormat stay procedural (PIL rectangles/points, integer
coords, binary alpha, no AA). Only the tile slots actually referenced
by the maps are painted; every other slot stays transparent.

Set ANTEA_DIR to point at the sprite folder; if it is missing the
script falls back to the original procedural furniture art.
"""

import os
from PIL import Image, ImageDraw

T = 16
import os
REPO_TILESETS = os.path.join(os.path.dirname(__file__), "..", "src", "assets", "tilesets")

# ---------------------------------------------------------------- palette
WOOD      = (201, 168, 117, 255)   # warm desk surface  #c9a875
WOOD_HI   = (220, 193, 145, 255)   # top highlight      #dcc391
WOOD_EDGE = (138, 107,  77, 255)   # darker edge        #8a6b4d
WOOD_DK   = (111,  82,  56, 255)   # shadow / legs      #6f5238
WOOD_GRN  = (176, 141,  94, 255)   # grain              #b08d5e
WOOD_FRNT = (172, 135,  96, 255)   # desk front face    #ac8760
DARKWOOD  = (90,  61,  43, 255)    # thin table slab    #5a3d2b
DARKWOOD2 = (74,  48,  32, 255)

WALL   = (232, 228, 218, 255)      # off-white wall     #e8e4da
WALL_S = (201, 195, 180, 255)      # wall shade         #c9c3b4
WALL_T = (181, 175, 161, 255)      # trim               #b5afa1
WALL_B = (143, 137, 125, 255)      # baseboard          #8f897d
WALL_O = (168, 162, 148, 255)      # outline            #a8a294

CARPET   = (125, 155, 134, 255)    # sage               #7d9b86
CARPET_D = (114, 146, 125, 255)    # darker check       #72927d
CARPET_S = (103, 131, 111, 255)    # speckle / junction #67836f
CARPET_L = (133, 162, 141, 255)    # faint highlight

GREY   = (154, 160, 168, 255)      # metal              #9aa0a8
GREY_D = (110, 116, 124, 255)      #                    #6e747c
GREY_L = (196, 200, 206, 255)      #                    #c4c8ce
GREY_XD= (74,  79,  87, 255)       # dark bezel/bin     #4a4f57
GREY_XXD=(53,  58,  65, 255)       #                    #353a41

SCR_OFF = (42, 47, 54, 255)        # monitor off        #2a2f36
SCR_ON  = (62, 224, 192, 255)      # teal glow          #3ee0c0
SCR_ON_D= (35, 179, 154, 255)      # code lines         #23b39a
SCR_ON_H= (139, 240, 220, 255)     # glow highlight

PLANT   = (74, 143, 90, 255)       #                    #4a8f5a
PLANT_D = (53, 112, 69, 255)       #                    #357045
PLANT_L = (102, 179, 116, 255)     #                    #66b374
POT     = (160, 106, 74, 255)      # terracotta         #a06a4a
POT_D   = (138, 90, 62, 255)       #                    #8a5a3e

RED    = (192, 57, 43, 255)        #                    #c0392b
RED_D  = (154, 40, 30, 255)
WHITE  = (242, 240, 232, 255)      #                    #f2f0e8
CREAM  = (236, 232, 222, 255)      # counter top        #ece8de
CREAM_S= (207, 201, 188, 255)

MAUVE   = (183, 110, 121, 255)     # sofa pink          #b76e79
MAUVE_D = (148, 86, 95, 255)       #                    #94565f
MAUVE_L = (205, 138, 148, 255)
MAUVE_XD= (125, 67, 77, 255)

MINT   = (159, 212, 168, 255)      # vending body       #9fd4a8
MINT_D = (127, 184, 138, 255)      #                    #7fb88a
MINT_L = (201, 236, 208, 255)
GLASS  = (34, 51, 44, 255)         # vending glass      #22332c
GLASS_L= (52, 74, 64, 255)

GLASS_WIN = (174, 212, 230, 255)   # window glass       #aed4e6
GLASS_WIN_D = (140, 185, 210, 255)

BOOK_COLORS = [(192,57,43,255),(58,110,165,255),(74,143,90,255),
               (212,160,23,255),(142,94,168,255),(200,120,60,255)]

CAN_COLORS = [(192,57,43,255),(58,110,165,255),(212,160,23,255),
              (62,224,192,255),(230,230,230,255),(142,94,168,255)]

# ---------------------------------------------------------------- helpers
def new_tile():
    return Image.new("RGBA", (T, T), (0, 0, 0, 0))

def rect(d, x, y, w, h, c):
    """Half-open rect [x,x+w) x [y,y+h)."""
    if w <= 0 or h <= 0:
        return
    d.rectangle([x, y, x + w - 1, y + h - 1], fill=c)

def px(d, x, y, c):
    d.point((x, y), fill=c)

def paste_tile(atlas, gid, firstgid, img):
    tid = gid - firstgid
    atlas.paste(img, ((tid % 16) * T, (tid // 16) * T))

def carve(scratch, sx, sy, w, h):
    """Crop a w*h block from scratch at (sx,sy) as a tile image."""
    t = new_tile()
    t.paste(scratch.crop((sx, sy, sx + w, sy + h)), (0, 0))
    return t

# ================================================================
#  office-tileset.png  (gids 1-512)
# ================================================================
def desk_unit():
    """48x32 desk: top surface row + front face row."""
    s = Image.new("RGBA", (48, 32), (0, 0, 0, 0))
    d = ImageDraw.Draw(s)
    # ---- top surface (y 0-15)
    rect(d, 0, 0, 48, 16, WOOD)
    rect(d, 0, 0, 48, 1, WOOD_HI)
    rect(d, 0, 15, 48, 1, WOOD_EDGE)
    # grain: horizontal short dashes
    for y in (4, 8, 12):
        rect(d, 4, y, 5, 1, WOOD_GRN)
        rect(d, 14, y, 5, 1, WOOD_GRN)
        rect(d, 25, y, 5, 1, WOOD_GRN)
        rect(d, 36, y, 5, 1, WOOD_GRN)
    # side edges on outer ends only
    rect(d, 0, 0, 1, 16, WOOD_EDGE)
    rect(d, 47, 0, 1, 16, WOOD_EDGE)
    # ---- front face (y 16-31)
    rect(d, 0, 16, 48, 3, WOOD_DK)          # under-top shadow
    rect(d, 0, 19, 48, 13, WOOD_FRNT)
    rect(d, 0, 19, 48, 1, WOOD)             # light line under shadow
    rect(d, 0, 31, 48, 1, WOOD_DK)
    rect(d, 0, 16, 1, 16, WOOD_EDGE)
    rect(d, 47, 16, 1, 16, WOOD_EDGE)
    # modesty seams every 16px + feet
    for x in (16, 32):
        rect(d, x, 22, 1, 9, WOOD_EDGE)
    for x in (2, 44):
        rect(d, x, 28, 3, 4, WOOD_DK)
    return s

def counter_top_unit():
    """48x16 white/cream counter top (tiles 38,39,40 + lone uses)."""
    s = Image.new("RGBA", (48, 16), (0, 0, 0, 0))
    d = ImageDraw.Draw(s)
    rect(d, 0, 0, 48, 16, CREAM)
    rect(d, 0, 0, 48, 1, (245, 242, 234, 255))
    rect(d, 0, 14, 48, 1, CREAM_S)
    rect(d, 0, 15, 48, 1, WALL_O)
    # subtle divider seam on the left of each tile
    for x in (0, 16, 32):
        rect(d, x, 1, 1, 13, (216, 210, 196, 255))
    return s

def thin_table():
    im = new_tile(); d = ImageDraw.Draw(im)
    rect(d, 0, 5, 16, 4, DARKWOOD)
    rect(d, 0, 5, 16, 1, (122, 90, 64, 255))
    rect(d, 1, 9, 2, 5, DARKWOOD2)
    rect(d, 13, 9, 2, 5, DARKWOOD2)
    return im

def plank(dots=True, end=False):
    """Conference-table wood plank; symmetric so H/V flips look fine."""
    im = new_tile(); d = ImageDraw.Draw(im)
    rect(d, 0, 0, 16, 16, WOOD)
    # stitched border -> reads as plank seams when tiled
    rect(d, 0, 0, 16, 1, WOOD_GRN)
    rect(d, 0, 15, 16, 1, WOOD_GRN)
    rect(d, 0, 0, 1, 16, WOOD_GRN)
    rect(d, 15, 0, 1, 16, WOOD_GRN)
    if end:
        rect(d, 0, 0, 1, 16, WOOD_EDGE)
        rect(d, 15, 0, 1, 16, WOOD_EDGE)
    # vertical grain
    rect(d, 5, 2, 1, 12, WOOD_GRN)
    rect(d, 10, 2, 1, 12, WOOD_GRN)
    if dots:
        for y in (3, 7, 11):
            px(d, 2, y, WOOD_EDGE)
            px(d, 13, y, WOOD_EDGE)
    return im

def drawer():
    im = new_tile(); d = ImageDraw.Draw(im)
    rect(d, 0, 0, 16, 16, WOOD)
    rect(d, 0, 0, 16, 1, WOOD_EDGE)
    rect(d, 0, 15, 16, 1, WOOD_EDGE)
    rect(d, 1, 2, 14, 1, WOOD_GRN)
    rect(d, 1, 13, 14, 1, WOOD_GRN)
    rect(d, 5, 6, 6, 2, WOOD_DK)      # handle
    rect(d, 5, 6, 6, 1, (150, 120, 86, 255))
    return im

def office_chair(body, shade, hi):
    """Top-down office chair: the agent sits facing UP toward the monitor, so
    the backrest is the dark band at the BOTTOM of the tile. Fill the tile —
    a chair that only uses the middle reads as a floating cushion."""
    im = new_tile(); d = ImageDraw.Draw(im)
    # 5-star base shadow
    rect(d, 4, 14, 8, 2, shade)
    px(d, 2, 15, shade); px(d, 13, 15, shade)
    px(d, 3, 14, shade); px(d, 12, 14, shade)
    # backrest (bottom, behind the sitter) — full width, chunky
    rect(d, 1, 10, 14, 4, shade)
    rect(d, 1, 10, 14, 1, hi)
    rect(d, 2, 11, 12, 2, body)
    # armrests
    rect(d, 0, 4, 2, 6, shade)
    rect(d, 14, 4, 2, 6, shade)
    rect(d, 0, 4, 2, 1, hi)
    rect(d, 14, 4, 2, 1, hi)
    # seat cushion — edge-to-edge
    rect(d, 2, 3, 12, 7, body)
    rect(d, 3, 4, 10, 5, hi)
    rect(d, 2, 3, 12, 1, hi)
    return im

def mauve_chair_back():
    """Full armchair seen from the front (gid 257)."""
    im = new_tile(); d = ImageDraw.Draw(im)
    rect(d, 2, 2, 12, 4, MAUVE_D)          # backrest
    rect(d, 2, 2, 12, 1, MAUVE_XD)
    rect(d, 2, 6, 12, 7, MAUVE)            # seat
    rect(d, 3, 7, 10, 5, MAUVE_L)
    rect(d, 2, 12, 12, 1, MAUVE_D)
    px(d, 3, 14, MAUVE_XD); px(d, 12, 14, MAUVE_XD)
    return im

def mauve_cushion():
    """Chair backrest+upper seat, top half of front-facing chair (gid 258).
    Fills the tile — reads as a lounge chair, not a pillow."""
    im = new_tile(); d = ImageDraw.Draw(im)
    # rolled backrest top
    rect(d, 1, 0, 14, 5, MAUVE_D)
    rect(d, 1, 0, 14, 1, MAUVE_XD)
    rect(d, 2, 1, 12, 2, MAUVE)          # cushion on the backrest
    # armrests run down the sides
    rect(d, 0, 4, 3, 12, MAUVE_D)
    rect(d, 13, 4, 3, 12, MAUVE_D)
    rect(d, 0, 4, 3, 1, MAUVE_XD)
    rect(d, 13, 4, 3, 1, MAUVE_XD)
    # seat
    rect(d, 3, 5, 10, 11, MAUVE)
    rect(d, 4, 6, 8, 9, MAUVE_L)
    return im

def mauve_chair_front():
    """Chair lower half: seat + feet (gid 274)."""
    im = new_tile(); d = ImageDraw.Draw(im)
    rect(d, 0, 0, 3, 10, MAUVE_D)         # armrest tails
    rect(d, 13, 0, 3, 10, MAUVE_D)
    rect(d, 3, 0, 10, 11, MAUVE)
    rect(d, 4, 0, 8, 9, MAUVE_L)
    rect(d, 3, 11, 10, 2, MAUVE_D)        # seat front edge
    rect(d, 3, 13, 3, 3, MAUVE_XD)        # feet
    rect(d, 10, 13, 3, 3, MAUVE_XD)
    return im

def mauve_armchair_side():
    im = new_tile(); d = ImageDraw.Draw(im)
    rect(d, 2, 2, 12, 12, MAUVE)
    rect(d, 2, 2, 12, 1, MAUVE_XD)
    rect(d, 2, 2, 3, 12, MAUVE_D)          # armrest
    rect(d, 8, 4, 6, 9, MAUVE_L)
    px(d, 3, 14, MAUVE_XD); px(d, 12, 14, MAUVE_XD)
    return im

def side_table():
    im = new_tile(); d = ImageDraw.Draw(im)
    rect(d, 3, 4, 10, 8, CREAM)
    rect(d, 3, 4, 10, 1, (245, 242, 234, 255))
    rect(d, 3, 11, 10, 1, CREAM_S)
    rect(d, 4, 12, 2, 3, GREY_D)
    rect(d, 10, 12, 2, 3, GREY_D)
    return im

def monitor_pair(on):
    """32x32 monitor -> 4 tiles."""
    s = Image.new("RGBA", (32, 32), (0, 0, 0, 0))
    d = ImageDraw.Draw(s)
    scr = SCR_ON if on else SCR_OFF
    # bezel + screen
    rect(d, 2, 3, 28, 21, GREY_XD)
    rect(d, 2, 3, 28, 1, (90, 96, 105, 255))
    rect(d, 4, 5, 24, 15, scr)
    if on:
        rect(d, 4, 5, 24, 1, SCR_ON_H)
        # code lines
        for y, w0 in ((8, 18), (11, 12), (14, 16), (17, 9)):
            rect(d, 6, y, w0, 1, SCR_ON_D)
        px(d, 24, 18, SCR_ON_H)
    else:
        rect(d, 4, 5, 24, 1, (56, 63, 71, 255))
        rect(d, 4, 19, 24, 1, (33, 38, 44, 255))
        px(d, 24, 7, (70, 78, 88, 255))
    # chin LED
    px(d, 15, 22, SCR_ON if on else (85, 90, 98, 255))
    # stand
    rect(d, 13, 24, 6, 4, GREY_XXD)
    rect(d, 9, 28, 14, 3, GREY_XD)
    rect(d, 9, 28, 14, 1, (90, 96, 105, 255))
    return [carve(s, 0, 0, 16, 16), carve(s, 16, 0, 16, 16),
            carve(s, 0, 16, 16, 16), carve(s, 16, 16, 16, 16)]

def window_pair():
    """32x16 two-pane window -> tiles 343,344."""
    s = Image.new("RGBA", (32, 16), (0, 0, 0, 0))
    d = ImageDraw.Draw(s)
    rect(d, 1, 1, 30, 14, GREY_D)          # outer frame
    rect(d, 2, 2, 28, 12, GLASS_WIN)
    rect(d, 15, 2, 2, 12, GREY_D)          # center mullion
    rect(d, 2, 7, 28, 2, GREY_D)           # horizontal rail
    # glass shading + shine
    rect(d, 3, 3, 5, 3, (200, 230, 244, 255))
    rect(d, 18, 3, 5, 3, (200, 230, 244, 255))
    rect(d, 3, 10, 12, 3, GLASS_WIN_D)
    rect(d, 18, 10, 12, 3, GLASS_WIN_D)
    for i in range(4):
        px(d, 5 + i, 5 - i if 5 - i >= 3 else 3, (235, 246, 252, 255))
    px(d, 9, 4, (235, 246, 252, 255)); px(d, 10, 3, (235, 246, 252, 255))
    return [carve(s, 0, 0, 16, 16), carve(s, 16, 0, 16, 16)]

def bookshelf_pair():
    """32x16 bookshelf with colorful spines -> tiles 207,208."""
    s = Image.new("RGBA", (32, 16), (0, 0, 0, 0))
    d = ImageDraw.Draw(s)
    rect(d, 0, 0, 32, 16, WOOD_EDGE)       # frame
    rect(d, 1, 1, 30, 14, (90, 70, 50, 255))  # dark interior
    # two shelves of books
    for row_y in (2, 9):
        x = 2
        i = 0
        while x < 29:
            w = 2 if (x // 3) % 2 == 0 else 3
            h = 4 + ((x + row_y) % 3)
            if x + w < 30 and (x + i) % 7 != 0:
                rect(d, x, row_y + (5 - h), w, h, BOOK_COLORS[(x + row_y) % len(BOOK_COLORS)])
            x += w + 1
            i += 1
        rect(d, 1, row_y + 5, 30, 1, WOOD_EDGE)   # shelf board
    return [carve(s, 0, 0, 16, 16), carve(s, 16, 0, 16, 16)]

def grey_cabinet(half):
    im = new_tile(); d = ImageDraw.Draw(im)
    rect(d, 0, 0, 16, 16, GREY_L)
    rect(d, 0, 0, 16, 1, (215, 219, 224, 255))
    rect(d, 0, 15, 16, 1, GREY_D)
    rect(d, 0, 0, 1, 16, GREY_D if half == "l" else GREY_L)
    rect(d, 15, 0, 1, 16, GREY_D if half == "r" else GREY_L)
    rect(d, 2, 7, 12, 1, GREY_D)           # drawer seam
    rect(d, 6, 4, 4, 1, GREY_XD)           # handle
    rect(d, 6, 10, 4, 1, GREY_XD)
    return im

def wall_ac(half):
    """White wall-mounted unit (AC/cabinet) - tiles 327,328."""
    im = new_tile(); d = ImageDraw.Draw(im)
    rect(d, 0, 3, 16, 10, (238, 240, 242, 255))
    rect(d, 0, 3, 16, 1, (215, 219, 224, 255))
    rect(d, 0, 12, 16, 1, GREY_D)
    rect(d, 1, 9, 14, 2, GREY)             # vent slit
    if half == "l":
        rect(d, 0, 3, 1, 10, GREY_D)
    else:
        rect(d, 15, 3, 1, 10, GREY_D)
        px(d, 13, 5, PLANT_L)              # status LED
    return im

def filing(half):
    """Filing cabinet / coffee machine base (329,330)."""
    im = new_tile(); d = ImageDraw.Draw(im)
    rect(d, 0, 0, 16, 16, GREY)
    rect(d, 0, 0, 16, 1, GREY_L)
    rect(d, 0, 15, 16, 1, GREY_D)
    if half == "l":
        rect(d, 0, 0, 1, 16, GREY_D)
        rect(d, 3, 3, 9, 5, GREY_XD)       # machine window/reservoir
        rect(d, 3, 8, 9, 1, GREY_XXD)
    else:
        rect(d, 15, 0, 1, 16, GREY_D)
        rect(d, 3, 4, 10, 1, GREY_D)       # drawer seam
        rect(d, 3, 9, 10, 1, GREY_D)
        rect(d, 7, 6, 3, 1, GREY_XD)
        rect(d, 7, 11, 3, 1, GREY_XD)
    return im

def partition(half):
    im = new_tile(); d = ImageDraw.Draw(im)
    rect(d, 0, 0, 16, 16, CREAM)
    rect(d, 0, 0, 16, 1, (245, 242, 234, 255))
    rect(d, 0, 15, 16, 1, CREAM_S)
    rect(d, 0, 14, 16, 1, (222, 216, 202, 255))
    if half == "l":
        rect(d, 0, 0, 1, 16, WALL_O)
    else:
        rect(d, 15, 0, 1, 16, WALL_O)
    return im

def appliance_white():
    """Water-cooler-ish appliance (281)."""
    im = new_tile(); d = ImageDraw.Draw(im)
    rect(d, 3, 2, 10, 13, (232, 233, 234, 255))
    rect(d, 3, 2, 10, 1, GREY_L)
    rect(d, 4, 4, 8, 5, (150, 190, 214, 255))   # blue bottle/window
    rect(d, 4, 4, 8, 1, (190, 220, 236, 255))
    rect(d, 4, 10, 8, 4, GREY_L)
    px(d, 5, 11, RED); px(d, 7, 11, (58, 110, 165, 255))
    rect(d, 3, 14, 10, 1, GREY_D)
    return im

def appliance_grey():
    """Tall grey appliance side (282)."""
    im = new_tile(); d = ImageDraw.Draw(im)
    rect(d, 4, 1, 9, 14, GREY)
    rect(d, 4, 1, 9, 1, GREY_L)
    rect(d, 4, 14, 9, 1, GREY_D)
    rect(d, 4, 1, 1, 14, GREY_D)
    rect(d, 6, 4, 5, 5, GREY_XD)
    rect(d, 6, 11, 5, 1, GREY_D)
    return im

def appliance_panel():
    """Grey appliance front / microwave-ish (285)."""
    im = new_tile(); d = ImageDraw.Draw(im)
    rect(d, 1, 3, 14, 11, GREY)
    rect(d, 1, 3, 14, 1, GREY_L)
    rect(d, 1, 13, 14, 1, GREY_D)
    rect(d, 3, 5, 7, 6, GREY_XXD)          # dark window
    rect(d, 3, 5, 7, 1, GREY_XD)
    rect(d, 11, 5, 2, 6, GREY_L)           # button column
    px(d, 11, 6, RED)
    return im

def microwave():
    """Grey counter/microwave box (269)."""
    im = new_tile(); d = ImageDraw.Draw(im)
    rect(d, 0, 4, 16, 10, GREY)
    rect(d, 0, 4, 16, 1, GREY_L)
    rect(d, 0, 13, 16, 1, GREY_D)
    rect(d, 2, 6, 9, 6, GREY_XXD)
    rect(d, 2, 6, 9, 1, GREY_XD)
    px(d, 13, 7, RED); px(d, 13, 9, GREY_L); px(d, 13, 11, GREY_L)
    return im

def small_tray():
    """Small white tray/object with red accent (276)."""
    im = new_tile(); d = ImageDraw.Draw(im)
    rect(d, 3, 8, 10, 5, (232, 233, 234, 255))
    rect(d, 3, 8, 10, 1, GREY_L)
    rect(d, 3, 12, 10, 1, GREY_D)
    rect(d, 4, 6, 4, 2, RED)               # red item on tray
    rect(d, 9, 7, 3, 1, GREY)
    return im

def dark_bin(variant=0):
    """Small dark trash bin (297,298)."""
    im = new_tile(); d = ImageDraw.Draw(im)
    rect(d, 4, 5, 8, 9, GREY_XD)
    rect(d, 3, 4, 10, 2, GREY_XXD)
    rect(d, 5, 7, 6, 1, GREY_XXD)
    if variant:
        rect(d, 5, 9, 6, 2, GREY_D)
    return im

def paper_or_mug():
    """Small white mug on the counter (301)."""
    im = new_tile(); d = ImageDraw.Draw(im)
    rect(d, 5, 7, 6, 7, WHITE)
    rect(d, 5, 7, 6, 1, (210, 206, 196, 255))
    rect(d, 6, 8, 4, 1, (140, 95, 60, 255))   # coffee surface
    rect(d, 11, 9, 2, 4, WHITE)               # handle
    px(d, 12, 10, WHITE)
    return im

def red_mug():
    """White mug with red band (411)."""
    im = new_tile(); d = ImageDraw.Draw(im)
    rect(d, 5, 6, 6, 8, WHITE)
    rect(d, 5, 6, 6, 2, RED)
    rect(d, 11, 8, 2, 4, WHITE)
    rect(d, 6, 13, 4, 1, (210, 206, 196, 255))
    return im

def keyboard():
    im = new_tile(); d = ImageDraw.Draw(im)
    rect(d, 2, 9, 12, 5, GREY_XXD)
    rect(d, 2, 9, 12, 1, GREY_XD)
    for yy in (10, 12):
        for x in range(3, 13, 2):
            px(d, x, yy, GREY_D)
    return im

def phone_device():
    """Small grey desk phone / device (313)."""
    im = new_tile(); d = ImageDraw.Draw(im)
    rect(d, 4, 8, 8, 6, GREY_XD)
    rect(d, 5, 9, 6, 2, (139, 200, 220, 255))
    rect(d, 5, 12, 6, 1, GREY_XXD)
    return im

def alarm_clock():
    im = new_tile(); d = ImageDraw.Draw(im)
    # bells
    rect(d, 3, 1, 3, 2, RED_D); rect(d, 10, 1, 3, 2, RED_D)
    # body ring
    rect(d, 3, 3, 10, 10, RED)
    rect(d, 2, 4, 12, 8, RED)
    # face
    rect(d, 4, 4, 8, 7, WHITE)
    rect(d, 5, 3, 6, 1, WHITE)
    # hands
    px(d, 8, 5, GREY_XXD); px(d, 8, 6, GREY_XXD); px(d, 8, 7, GREY_XXD)
    px(d, 9, 8, GREY_XXD); px(d, 10, 8, GREY_XXD)
    # feet
    px(d, 4, 13, RED_D); px(d, 11, 13, RED_D)
    return im

def plant_pot_small():
    """Tiny potted plant for wall ledge (370)."""
    im = new_tile(); d = ImageDraw.Draw(im)
    rect(d, 6, 4, 4, 2, PLANT_L)
    rect(d, 7, 2, 2, 4, PLANT)
    px(d, 5, 3, PLANT); px(d, 10, 3, PLANT)
    rect(d, 5, 9, 6, 5, POT)
    rect(d, 5, 9, 6, 1, POT_D)
    rect(d, 6, 13, 4, 1, POT_D)
    return im

def plant_tall(variant=0):
    """Tall potted plant (431,451,467)."""
    im = new_tile(); d = ImageDraw.Draw(im)
    # leaves: tapered column
    c = [PLANT, PLANT_D, PLANT_L][variant % 3]
    rect(d, 7, 1, 2, 3, c)
    rect(d, 6, 3, 4, 3, c)
    rect(d, 5, 5, 6, 4, PLANT)
    rect(d, 4, 7, 8, 3, PLANT_D)
    px(d, 6, 2, PLANT_L); px(d, 9, 4, PLANT_L); px(d, 5, 6, PLANT_L)
    # pot
    rect(d, 4, 11, 8, 4, POT)
    rect(d, 4, 11, 8, 1, POT_D)
    rect(d, 5, 14, 6, 1, POT_D)
    return im

def plant_mid(variant=0):
    """Mid bushy plant (432,452,468)."""
    im = new_tile(); d = ImageDraw.Draw(im)
    c = [PLANT, PLANT_L, PLANT_D][variant % 3]
    rect(d, 4, 4, 8, 2, c)
    rect(d, 3, 6, 10, 3, PLANT)
    rect(d, 4, 9, 8, 2, PLANT_D)
    px(d, 6, 3, PLANT_L); px(d, 9, 5, PLANT_L); px(d, 5, 7, PLANT_D)
    rect(d, 5, 11, 6, 4, POT)
    rect(d, 5, 11, 6, 1, POT_D)
    return im

def plant_leafy(variant=0):
    """Wide leafy floor plant (470,471,472)."""
    im = new_tile(); d = ImageDraw.Draw(im)
    c = [PLANT_D, PLANT, PLANT_L][variant % 3]
    rect(d, 6, 1, 4, 3, c)
    rect(d, 3, 3, 10, 3, PLANT)
    rect(d, 2, 6, 12, 3, c)
    rect(d, 4, 9, 8, 2, PLANT_D)
    px(d, 5, 2, PLANT_L); px(d, 10, 4, PLANT_L); px(d, 4, 7, PLANT_L)
    rect(d, 5, 11, 6, 4, POT)
    rect(d, 5, 11, 6, 1, POT_D)
    return im

def flip_chart():
    """White flip-chart / panel on stand (395)."""
    im = new_tile(); d = ImageDraw.Draw(im)
    # legs
    px(d, 6, 12, GREY_D); px(d, 9, 12, GREY_D)
    rect(d, 5, 13, 1, 3, GREY_D); rect(d, 10, 13, 1, 3, GREY_D)
    # board
    rect(d, 3, 1, 10, 11, (90, 96, 105, 255))
    rect(d, 4, 2, 8, 9, WHITE)
    # content lines + blue accent edge
    rect(d, 4, 2, 1, 9, (150, 190, 214, 255))
    for y in (4, 6, 8):
        rect(d, 6, y, 4, 1, GREY_L)
    return im

# ================================================================
#  a5-office-floors-walls.png  (gids 513-1024)
# ================================================================
def wall(edges=(), baseboard=False, capital=False):
    """Off-white wall tile. edges = subset of 'l','r','t'."""
    im = new_tile(); d = ImageDraw.Draw(im)
    rect(d, 0, 0, 16, 16, WALL)
    if "t" in edges:
        rect(d, 0, 0, 16, 1, WALL_O)
        rect(d, 0, 1, 16, 1, WALL_T)
    if "l" in edges:
        rect(d, 0, 0, 1, 16, WALL_T)
        rect(d, 1, 0, 1, 16, WALL_S)
    if "r" in edges:
        rect(d, 15, 0, 1, 16, WALL_T)
        rect(d, 14, 0, 1, 16, WALL_S)
    if capital:   # column capital: darker top cap
        rect(d, 0, 0, 16, 2, WALL_T)
        rect(d, 0, 2, 16, 1, WALL_S)
    if baseboard:
        rect(d, 0, 13, 16, 1, WALL_S)
        rect(d, 0, 14, 16, 2, WALL_B)
    return im

def carpet(dark=False, seed=0):
    im = new_tile(); d = ImageDraw.Draw(im)
    base = CARPET_D if dark else CARPET
    rect(d, 0, 0, 16, 16, base)
    # a couple of sparse speckles (deterministic, quiet)
    px(d, (4 + seed * 5) % 12 + 2, 5 + (seed % 3), CARPET_S)
    px(d, (9 + seed * 3) % 12 + 2, 11 - (seed % 3),
       CARPET_L if not dark else CARPET_S)
    # small corner mark -> subtle diamond dot at every tile junction
    for cx, cy in ((0, 0), (15, 0), (0, 15), (15, 15)):
        px(d, cx, cy, CARPET_S)
    return im

# ================================================================
#  interiors.png  (gids 1025-2448)
# ================================================================
def vending_machine():
    """48x48 composite vending machine occupying tiles:
       row0: 1041 1042 1043
       row1: 1044 1076 1061
       row2: 1073 1074 1075
    """
    s = Image.new("RGBA", (48, 48), (0, 0, 0, 0))
    d = ImageDraw.Draw(s)
    # body
    rect(d, 1, 0, 46, 46, MINT)
    rect(d, 1, 0, 46, 2, MINT_L)          # top cap
    rect(d, 1, 2, 46, 1, MINT_D)
    rect(d, 1, 0, 2, 46, MINT_D)          # side shading
    rect(d, 45, 0, 2, 46, MINT_D)
    # glass front (x 8..33, y 6..33)
    rect(d, 8, 6, 26, 28, GREY_XXD)
    rect(d, 9, 7, 24, 26, GLASS)
    # can rows: 4 rows x 4 cols
    for ry in range(4):
        for cx in range(4):
            x = 11 + cx * 6
            y = 9 + ry * 6
            rect(d, x, y, 4, 4, CAN_COLORS[(ry * 4 + cx) % len(CAN_COLORS)])
            px(d, x, y, (240, 240, 240, 255))
        rect(d, 9, 14 + ry * 6, 24, 1, GLASS_L)   # shelf lines
    # right side panel (x 36..42)
    rect(d, 36, 6, 7, 28, MINT_D)
    rect(d, 37, 8, 5, 10, GREY_XXD)       # selection panel
    for i in range(3):
        rect(d, 38, 10 + i * 3, 3, 1, GREY_L)
    rect(d, 37, 21, 5, 6, GREY_XD)        # coin slot
    px(d, 39, 24, (20, 24, 22, 255))
    px(d, 38, 29, RED)
    # pickup slot + base
    rect(d, 9, 35, 24, 6, GREY_XD)
    rect(d, 10, 36, 22, 4, GREY_XXD)
    rect(d, 1, 42, 46, 4, (94, 110, 98, 255))
    rect(d, 1, 42, 46, 1, MINT_D)
    rect(d, 4, 46, 6, 2, GREY_XXD)
    rect(d, 38, 46, 6, 2, GREY_XXD)
    order = [(0, 0), (16, 0), (32, 0), (0, 16), (16, 16), (32, 16),
             (0, 32), (16, 32), (32, 32)]
    return [carve(s, x, y, 16, 16) for x, y in order]

def doormat_unit():
    """32x32 doormat -> tiles 1698,1699 / 1714,1715."""
    s = Image.new("RGBA", (32, 32), (0, 0, 0, 0))
    d = ImageDraw.Draw(s)
    rect(d, 0, 0, 32, 32, (201, 180, 138, 255))     # tan field
    rect(d, 0, 0, 32, 2, (168, 148, 104, 255))
    rect(d, 0, 30, 32, 2, (168, 148, 104, 255))
    rect(d, 0, 0, 2, 32, (168, 148, 104, 255))
    rect(d, 30, 0, 2, 32, (168, 148, 104, 255))
    # inner stitch border
    for x in range(4, 28, 2):
        px(d, x, 4, (184, 166, 126, 255)); px(d, x, 27, (184, 166, 126, 255))
    for y in range(4, 28, 2):
        px(d, 4, y, (184, 166, 126, 255)); px(d, 27, y, (184, 166, 126, 255))
    # weave dots
    for y in range(8, 26, 4):
        for x in range(8, 26, 4):
            px(d, x, y, (184, 166, 126, 255))
    return [carve(s, 0, 0, 16, 16), carve(s, 16, 0, 16, 16),
            carve(s, 0, 16, 16, 16), carve(s, 16, 16, 16, 16)]

def door_tile(part):
    """Wood door segments stacked vertically (2248 top,2264 mid,2280 bottom)."""
    im = new_tile(); d = ImageDraw.Draw(im)
    rect(d, 0, 0, 16, 16, WOOD)                  # door leaf
    rect(d, 0, 0, 2, 16, WOOD_EDGE)              # frame sides
    rect(d, 14, 0, 2, 16, WOOD_EDGE)
    if part == "top":
        rect(d, 0, 0, 16, 1, WOOD_DK)
        rect(d, 4, 3, 8, 9, WOOD_FRNT)           # upper panel
        rect(d, 4, 3, 8, 1, WOOD_DK); rect(d, 4, 11, 8, 1, WOOD_DK)
        rect(d, 4, 3, 1, 9, WOOD_DK); rect(d, 11, 3, 1, 9, WOOD_DK)
    elif part == "mid":
        rect(d, 4, 2, 8, 12, WOOD_FRNT)
        rect(d, 4, 2, 8, 1, WOOD_DK); rect(d, 4, 13, 8, 1, WOOD_DK)
        rect(d, 4, 2, 1, 12, WOOD_DK); rect(d, 11, 2, 1, 12, WOOD_DK)
        px(d, 13, 8, (60, 40, 25, 255))          # handle dot
        px(d, 13, 7, (240, 235, 225, 255))
    else:
        rect(d, 4, 2, 8, 10, WOOD_FRNT)
        rect(d, 4, 2, 8, 1, WOOD_DK); rect(d, 4, 11, 8, 1, WOOD_DK)
        rect(d, 4, 2, 1, 10, WOOD_DK); rect(d, 11, 2, 1, 10, WOOD_DK)
        rect(d, 0, 15, 16, 1, WOOD_DK)
    return im

def coffee_counter(which):
    """Row-11 counter pieces: 1026 machine-left, 1027 plain."""
    im = new_tile(); d = ImageDraw.Draw(im)
    rect(d, 0, 8, 16, 8, GREY)                    # counter slab
    rect(d, 0, 8, 16, 1, GREY_L)
    rect(d, 0, 15, 16, 1, GREY_D)
    if which == "machine":
        rect(d, 3, 1, 10, 8, (58, 62, 70, 255))   # coffee machine block
        rect(d, 3, 1, 10, 1, (90, 96, 105, 255))
        rect(d, 5, 5, 6, 3, GREY_XXD)             # nozzle area
        px(d, 7, 9, RED)
    else:
        rect(d, 2, 6, 12, 2, GREY_D)              # shelf line detail
    return im

def wood_counter_top():
    im = new_tile(); d = ImageDraw.Draw(im)
    rect(d, 0, 0, 16, 16, WOOD)
    rect(d, 0, 0, 16, 1, WOOD_HI)
    rect(d, 0, 13, 16, 1, WOOD_GRN)
    rect(d, 0, 14, 16, 2, WOOD_EDGE)
    for y in (5, 9):
        rect(d, 2, y, 4, 1, WOOD_GRN)
        rect(d, 10, y, 4, 1, WOOD_GRN)
    return im

# ================================================================
#  Antea "Office Furniture Pixel Art" (CC-BY) sprites
#
#  Furniture slots are cut/composited from these sprites instead of
#  the procedural drawings above.  All resizing uses NEAREST so the
#  pixel grid and binary alpha survive intact.
# ================================================================
ANTEA_DIR = os.environ.get(
    "ANTEA_DIR",
    r"D:\project\super-agent\cc0-assets\antea\Office-Furniture-Pixel-Art"
    r"\Office-Furniture-Pixel-Art")
HAVE_ANTEA = os.path.isdir(ANTEA_DIR)

_ANTEA_CACHE = {}

def antea(name):
    """Load (and cache) an Antea sprite as RGBA."""
    if name not in _ANTEA_CACHE:
        _ANTEA_CACHE[name] = Image.open(
            os.path.join(ANTEA_DIR, name)).convert("RGBA")
    return _ANTEA_CACHE[name]

def antea_tile(name, box=None, size=(T, T)):
    """Cut a tile from an Antea sprite at NATIVE scale — never resized.
    `box` picks the source region; if the result is larger than `size` the
    centred window is taken, if smaller it is pasted centred on a
    transparent canvas."""
    im = antea(name)
    if box is not None:
        im = im.crop(box)
    tw, th = size
    if im.width > tw or im.height > th:
        x0 = max(0, (im.width - tw) // 2)
        y0 = max(0, (im.height - th) // 2)
        im = im.crop((x0, y0, x0 + tw, y0 + th))
    if im.size != tuple(size):
        out = new_tile()
        out.paste(im, ((tw - im.width) // 2, (th - im.height) // 2), im)
        im = out
    return im

# Antea palette samples (dark navy desk line, maroon vending line)
A_SURF   = (74, 78, 104, 255)    # desktop
A_EDGE   = (89, 93, 121, 255)    # desktop edge
A_BAND   = (53, 56, 76, 255)     # front shadow band
A_BAND2  = (58, 62, 85, 255)
A_LEG    = (63, 67, 89, 255)     # legs
A_LEG_D  = (44, 47, 67, 255)
A_VEND   = (111, 34, 54, 255)    # vending body maroon
A_VEND_D = (72, 28, 46, 255)
A_VEND_X = (49, 19, 31, 255)     # kick / feet

def desk_unit_antea():
    """48x32 worker-desk stamp built on Antea Desk.png.

    The sprite is a whole workstation (monitor + keyboard + phone on a
    dark desk with legs).  The bottom 24 rows are pasted into the right
    two columns so the desktop band straddles the tile seam like the old
    stamp; the remaining monitor stub sits under the app's own 2x2
    monitor tiles.  Column 1 is a matching extension drawn in the same
    palette (flat surface, side edge, one leg).
    """
    s = Image.new("RGBA", (48, 32), (0, 0, 0, 0))
    body = antea("Desk.png").crop((0, 8, 32, 32))   # drop monitor top
    s.paste(body, (16, 8), body)
    d = ImageDraw.Draw(s)
    rect(d, 0, 13, 16, 1, A_EDGE)     # desktop back edge
    rect(d, 0, 14, 16, 9, A_SURF)     # desktop interior
    rect(d, 0, 23, 16, 1, A_EDGE)     # desktop front edge
    rect(d, 0, 24, 16, 1, A_BAND)     # front shadow bands
    rect(d, 0, 25, 16, 1, A_BAND2)
    rect(d, 0, 13, 1, 13, A_EDGE)     # left side edge
    rect(d, 1, 26, 2, 4, A_LEG)       # left leg
    rect(d, 3, 26, 1, 4, A_LEG_D)
    return s

def monitor_pair_antea(on):
    """32x32 monitor: bezel+screen cropped from Antea Desk.png, upscaled
    2x (NEAREST), on a stand drawn in the desk palette.  OFF darkens the
    screen ramp to a dim slate; ON remaps it to the app's teal glow."""
    src = antea("Desk.png").crop((3, 1, 17, 12))      # 14x11 monitor
    screen = {(84, 97, 125): 0, (95, 108, 138): 1,
              (111, 125, 157): 2, (143, 154, 179): 3}
    ramp = ([(26, 110, 96), (35, 179, 154), (62, 224, 192), (139, 240, 220)]
            if on else
            [(34, 40, 49), (43, 51, 61), (53, 63, 76), (68, 80, 96)])
    p = src.load()
    for y in range(src.height):
        for x in range(src.width):
            r, g, b, a = p[x, y]
            if a and (r, g, b) in screen:
                p[x, y] = (*ramp[screen[(r, g, b)]], a)
    s = Image.new("RGBA", (32, 32), (0, 0, 0, 0))
    s.paste(src, ((32 - src.width) // 2, (26 - src.height) // 2), src)
    d = ImageDraw.Draw(s)
    rect(d, 13, 25, 6, 4, A_BAND)          # stand stem
    rect(d, 9, 29, 14, 2, A_LEG)           # base
    rect(d, 9, 29, 14, 1, A_EDGE)
    return [carve(s, 0, 0, 16, 16), carve(s, 16, 0, 16, 16),
            carve(s, 0, 16, 16, 16), carve(s, 16, 16, 16, 16)]

def antea_plank(end=False):
    """Conference-table plank in Antea's dark desk palette; symmetric so
    the map's H/V/D flips render the same."""
    im = new_tile(); d = ImageDraw.Draw(im)
    rect(d, 0, 0, 16, 16, A_SURF)
    rect(d, 0, 0, 16, 1, A_BAND2)
    rect(d, 0, 15, 16, 1, A_BAND2)
    rect(d, 0, 0, 1, 16, A_BAND2)
    rect(d, 15, 0, 1, 16, A_BAND2)
    if end:
        rect(d, 0, 0, 1, 16, A_EDGE)
        rect(d, 15, 0, 1, 16, A_EDGE)
    rect(d, 5, 2, 1, 12, A_BAND2)
    rect(d, 10, 2, 1, 12, A_BAND2)
    for y in (3, 7, 11):
        px(d, 2, y, A_EDGE)
        px(d, 13, y, A_EDGE)
    return im

def antea_drawer():
    """Table end-cap / drawer in Antea's dark desk palette (symmetric)."""
    im = new_tile(); d = ImageDraw.Draw(im)
    rect(d, 0, 0, 16, 16, A_SURF)
    rect(d, 0, 0, 16, 1, A_EDGE)
    rect(d, 0, 15, 16, 1, A_EDGE)
    rect(d, 0, 0, 1, 16, A_EDGE)
    rect(d, 15, 0, 1, 16, A_EDGE)
    rect(d, 1, 2, 14, 1, A_BAND2)
    rect(d, 1, 13, 14, 1, A_BAND2)
    rect(d, 5, 6, 6, 2, A_LEG_D)          # handle
    rect(d, 5, 6, 6, 1, A_EDGE)
    return im

def counter_tile_antea():
    """16x16 counter in the Small-Table palette (dark navy slab)."""
    im = new_tile(); d = ImageDraw.Draw(im)
    rect(d, 0, 0, 16, 1, (116, 138, 166, 255))   # top highlight
    rect(d, 0, 1, 16, 9, A_SURF)                 # surface
    rect(d, 0, 10, 16, 4, A_BAND2)               # front face
    rect(d, 0, 14, 16, 2, A_LEG_D)               # kick
    return im

def coffee_counter_antea():
    """Counter tile with the Antea coffee machine standing on it."""
    im = counter_tile_antea()
    m = antea("Coffee-Machine.png").crop((0, 1, 16, 15))
    im.paste(m, (0, 0), m)
    return im

def plinth_tile():
    """Bottom row under the vending machine / water dispenser."""
    im = new_tile(); d = ImageDraw.Draw(im)
    rect(d, 0, 0, 16, 1, A_VEND_D)
    rect(d, 0, 1, 16, 11, A_VEND)
    rect(d, 0, 12, 16, 4, A_VEND_X)      # kick plate
    return im

def vending_block_antea():
    """48x48 refreshment nook.

    Antea Vending-Machine.png (32x32) fills the left 2x2; the right
    column gets a wall shelf (top) and the water dispenser (middle);
    the bottom row is a matching kick plate.  gid order per map:
      1041 1042 1043
      1044 1076 1061     (1076 is VD-flipped in the map -> stored
      1073 1074 1075      pre-flipped so it renders upright)
    """
    vend = antea("Vending-Machine.png")
    tl = vend.crop((0, 0, 16, 16))
    tr = vend.crop((16, 0, 32, 16))
    bl = vend.crop((0, 16, 16, 32))
    br = vend.crop((16, 16, 32, 32))
    # cancel the map's diagonal+vertical flip on the centre tile
    br_pre = br.transpose(Image.FLIP_TOP_BOTTOM).transpose(Image.TRANSPOSE)
    shelf = antea_tile("Wall-Shelf.png")
    disp = antea_tile("Water-Dispenser.png")          # 32x32 -> 16x16
    plinth = plinth_tile()
    return [tl, tr, shelf, bl, br_pre, disp, plinth, plinth, plinth]

# ================================================================
#  build atlases
# ================================================================
def build_office():
    img = Image.new("RGBA", (256, 512), (0, 0, 0, 0))
    F = 1

    # --- desk unit (2,3,4 top / 18,19,20 front): Antea Desk.png composite
    du = desk_unit_antea() if HAVE_ANTEA else desk_unit()
    for i, g in enumerate((2, 3, 4)):
        paste_tile(img, g, F, carve(du, i * 16, 0, 16, 16))
    for i, g in enumerate((18, 19, 20)):
        paste_tile(img, g, F, carve(du, i * 16, 16, 16, 16))

    # --- 54,55,56 : counter front faces (map order 54,56,55 -> l,mid,r)
    dup = desk_unit()
    paste_tile(img, 54, F, carve(dup, 0, 16, 16, 16))
    paste_tile(img, 56, F, carve(dup, 16, 16, 16, 16))
    paste_tile(img, 55, F, carve(dup, 32, 16, 16, 16))

    # --- 38,39,40 : white counter tops (identical, seam dividers)
    cu = counter_top_unit()
    for i, g in enumerate((38, 39, 40)):
        paste_tile(img, g, F, carve(cu, i * 16, 0, 16, 16))

    paste_tile(img, 1, F, thin_table())

    if not HAVE_ANTEA:
        # ---- procedural fallback (no Antea sprites on disk) ----
        paste_tile(img, 161, F, drawer())
        paste_tile(img, 162, F, drawer())
        for g in (177, 178, 229):
            paste_tile(img, g, F, plank(end=True))
        paste_tile(img, 193, F, plank(end=True))
        paste_tile(img, 194, F, plank(end=True))
        paste_tile(img, 191, F, partition("l"))
        paste_tile(img, 192, F, partition("r"))
        bs = bookshelf_pair()
        paste_tile(img, 207, F, bs[0]); paste_tile(img, 208, F, bs[1])
        paste_tile(img, 223, F, grey_cabinet("l"))
        paste_tile(img, 224, F, grey_cabinet("r"))
        paste_tile(img, 257, F, mauve_chair_back())
        paste_tile(img, 258, F, mauve_cushion())
        paste_tile(img, 260, F, mauve_armchair_side())
        paste_tile(img, 266, F, side_table())
        paste_tile(img, 274, F, mauve_chair_front())
        paste_tile(img, 269, F, microwave())
        paste_tile(img, 276, F, small_tray())
        paste_tile(img, 281, F, appliance_white())
        paste_tile(img, 282, F, appliance_grey())
        paste_tile(img, 285, F, appliance_panel())
        paste_tile(img, 289, F, office_chair((110, 78, 50, 255), (70, 48, 28, 255), (150, 116, 80, 255)))
        paste_tile(img, 305, F, office_chair(GREY, GREY_XD, GREY_L))
        paste_tile(img, 297, F, dark_bin(0))
        paste_tile(img, 298, F, dark_bin(1))
        paste_tile(img, 301, F, paper_or_mug())
        paste_tile(img, 313, F, phone_device())
        paste_tile(img, 327, F, wall_ac("l"))
        paste_tile(img, 328, F, wall_ac("r"))
        paste_tile(img, 329, F, filing("l"))
        paste_tile(img, 330, F, filing("r"))
        win = window_pair()
        paste_tile(img, 343, F, win[0]); paste_tile(img, 344, F, win[1])
        paste_tile(img, 349, F, keyboard())
        paste_tile(img, 354, F, alarm_clock())
        paste_tile(img, 370, F, plant_pot_small())
        paste_tile(img, 395, F, flip_chart())
        paste_tile(img, 411, F, red_mug())
        off = monitor_pair(False)
        on = monitor_pair(True)
        for g, t in zip((365, 366, 381, 382), off):
            paste_tile(img, g, F, t)
        for g, t in zip((367, 368, 383, 384), on):
            paste_tile(img, g, F, t)
        paste_tile(img, 431, F, plant_tall(0))
        paste_tile(img, 451, F, plant_tall(1))
        paste_tile(img, 467, F, plant_tall(2))
        paste_tile(img, 432, F, plant_mid(0))
        paste_tile(img, 452, F, plant_mid(1))
        paste_tile(img, 468, F, plant_mid(2))
        paste_tile(img, 470, F, plant_leafy(0))
        paste_tile(img, 471, F, plant_leafy(1))
        paste_tile(img, 472, F, plant_leafy(2))
        return img

    # ---- Antea CC-BY furniture ----
    # conference table: dark navy planks + end caps (map H/V/D flips ->
    # symmetric designs only)
    paste_tile(img, 161, F, antea_drawer())
    paste_tile(img, 162, F, antea_drawer())
    for g in (177, 178, 229):
        paste_tile(img, g, F, antea_plank(end=True))
    paste_tile(img, 193, F, antea_plank(end=True))
    paste_tile(img, 194, F, antea_plank(end=True))

    # wall shelves above the storage nook
    for g in (191, 192):
        paste_tile(img, g, F, antea_tile("Wall-Shelf.png"))

    # bookshelf pair + small filing cabinets below
    for g in (207, 208):
        paste_tile(img, g, F, antea_tile("Bookshelf.png"))
    for g in (223, 224):
        paste_tile(img, g, F, antea_tile("Filing-Cabinet-Small.png"))

    # conference chairs: Boss-Chair backrest (upper) / seat+base (lower),
    # a left-facing side chair, and small side tables
    boss_top = antea_tile("Boss-Chair.png", (8, 4, 24, 20))
    boss_bot = antea_tile("Boss-Chair.png", (8, 16, 24, 32))
    paste_tile(img, 257, F, boss_top)
    paste_tile(img, 258, F, boss_top)
    paste_tile(img, 274, F, boss_bot)
    paste_tile(img, 260, F, antea_tile("Chair-2.png"))
    paste_tile(img, 266, F, antea_tile("Small-Table.png"))
    paste_tile(img, 276, F, antea_tile("Small-Table.png"))

    # appliances: copier, water dispenser, tall cabinet, printer stand —
    # 32x32 sources are cropped, not resized; box = most readable window
    paste_tile(img, 269, F, antea_tile("Big-Office-Printer.png", (7, 4, 23, 20)))
    paste_tile(img, 281, F, antea_tile("Water-Dispenser.png", (3, 1, 19, 17)))
    paste_tile(img, 282, F, antea_tile("Filing-Cabinet-Tall.png", (8, 8, 24, 24)))
    paste_tile(img, 285, F, antea_tile("Printer-Furniture.png", (8, 8, 24, 24)))

    # worker chairs + bins + desk smalls
    ch = antea_tile("Chair.png")
    paste_tile(img, 289, F, ch)
    paste_tile(img, 305, F, ch)
    paste_tile(img, 297, F, antea_tile("Bin.png"))
    paste_tile(img, 298, F,
               antea_tile("Bin.png").transpose(Image.FLIP_LEFT_RIGHT))
    paste_tile(img, 301, F, paper_or_mug())
    paste_tile(img, 313, F, antea_tile("Printer.png"))

    # wall-mounted shelves + windows (windows stay procedural)
    for g in (327, 328):
        paste_tile(img, g, F, antea_tile("Wall-Shelf.png"))
    win = window_pair()
    paste_tile(img, 343, F, win[0]); paste_tile(img, 344, F, win[1])
    paste_tile(img, 349, F, antea_tile("Books.png"))
    paste_tile(img, 354, F, antea_tile("Wall-Clock.png"))
    paste_tile(img, 370, F, antea_tile("Small-Plant.png"))
    paste_tile(img, 395, F, antea_tile("Wall-Graph.png"))
    paste_tile(img, 411, F, antea_tile("Wall-Note.png"))

    # items on the break counter: coffee machine + paper tray
    paste_tile(img, 329, F, antea_tile("Coffee-Machine.png"))
    paste_tile(img, 330, F, antea_tile("Papers.png"))

    # monitors (2x2) cropped out of Antea Desk.png
    off = monitor_pair_antea(False)
    on = monitor_pair_antea(True)
    for g, t in zip((365, 366, 381, 382), off):
        paste_tile(img, g, F, t)
    for g, t in zip((367, 368, 383, 384), on):
        paste_tile(img, g, F, t)

    # plants: desk pots + floor plants split/downscaled from Big-Plant
    paste_tile(img, 431, F, antea_tile("Small-Plant.png"))
    paste_tile(img, 432, F, antea_tile("Small-Plant.png"))
    bp_top = antea_tile("Big-Plant.png", (8, 0, 24, 16))
    bp_bot = antea_tile("Big-Plant.png", (8, 16, 24, 32))
    for g in (451, 452):
        paste_tile(img, g, F, bp_top)
    for g in (467, 468):
        paste_tile(img, g, F, bp_bot)
    for g in (470, 471, 472):
        paste_tile(img, g, F, antea_tile("Big-Plant.png"))

    return img

def build_a5():
    img = Image.new("RGBA", (256, 512), (0, 0, 0, 0))
    F = 513
    # exterior / interior wall band
    paste_tile(img, 514, F, wall(edges=("t", "l")))
    paste_tile(img, 517, F, wall(edges=("t", "r")))
    paste_tile(img, 522, F, wall(edges=("t",)))
    paste_tile(img, 530, F, wall(edges=("l",)))
    paste_tile(img, 533, F, wall(edges=("r",)))
    paste_tile(img, 554, F, wall())
    paste_tile(img, 570, F, wall(baseboard=True))
    paste_tile(img, 578, F, wall(edges=("l",), baseboard=True))
    paste_tile(img, 579, F, wall(baseboard=True))
    paste_tile(img, 581, F, wall(edges=("r",), baseboard=True))
    # columns / pillars
    paste_tile(img, 611, F, wall(capital=True, baseboard=True))
    paste_tile(img, 643, F, wall(edges=("l", "r"), baseboard=True))
    # carpet 2x2 checker
    paste_tile(img, 783, F, carpet(dark=False, seed=0))
    paste_tile(img, 784, F, carpet(dark=True, seed=1))
    paste_tile(img, 799, F, carpet(dark=False, seed=2))
    paste_tile(img, 800, F, carpet(dark=True, seed=3))
    return img

def build_interiors():
    img = Image.new("RGBA", (256, 1424), (0, 0, 0, 0))
    F = 1025
    if HAVE_ANTEA:
        # cafe counter row: counter + Antea coffee machine + counter
        c = counter_tile_antea()
        paste_tile(img, 1029, F, c)
        paste_tile(img, 1026, F, coffee_counter_antea())
        paste_tile(img, 1027, F, c)
        # refreshment nook 3x3: vending + shelf + water dispenser + plinth
        vb = vending_block_antea()
        for g, t in zip((1041, 1042, 1043, 1044, 1076, 1061,
                         1073, 1074, 1075), vb):
            paste_tile(img, g, F, t)
        # hutch pair -> a 2-wide bookcase
        paste_tile(img, 1650, F, antea_tile("Bookshelf.png"))
    else:
        # coffee counter row
        paste_tile(img, 1029, F, wood_counter_top())
        paste_tile(img, 1026, F, coffee_counter("machine"))
        paste_tile(img, 1027, F, coffee_counter("plain"))

        # vending composite 3x3
        vm = vending_machine()
        for g, t in zip((1041, 1042, 1043, 1044, 1076, 1061, 1073, 1074, 1075), vm):
            paste_tile(img, g, F, t)

        # hutch: the map uses two adjacent 1650 tiles, so the single tile
        # is drawn to tile seamlessly when repeated side by side.
        paste_tile(img, 1650, F, hutch_as_single())
    # doormat 2x2 (no Antea equivalent)
    dm = doormat_unit()
    for g, t in zip((1698, 1699, 1714, 1715), dm):
        paste_tile(img, g, F, t)
    # door column
    paste_tile(img, 2248, F, door_tile("top"))
    paste_tile(img, 2264, F, door_tile("mid"))
    paste_tile(img, 2280, F, door_tile("bottom"))
    return img

def hutch_as_single():
    """16x16 hutch tile; two adjacent copies tile into the 2-wide cabinet."""
    im = new_tile(); d = ImageDraw.Draw(im)
    rect(d, 0, 0, 16, 16, (61, 74, 63, 255))
    rect(d, 1, 1, 14, 6, (160, 140, 100, 255))
    for x in range(2, 14, 4):
        h = 2 + (x % 3)
        rect(d, x, 6 - h, 3, h, CAN_COLORS[x % len(CAN_COLORS)])
    rect(d, 1, 7, 14, 1, (61, 74, 63, 255))
    rect(d, 1, 8, 14, 6, (42, 52, 45, 255))
    for ry in range(2):
        for cx in range(3):
            x = 3 + cx * 4
            y = 9 + ry * 3
            px(d, x, y, CAN_COLORS[(cx + ry * 2) % len(CAN_COLORS)])
            px(d, x + 1, y, CAN_COLORS[(cx + ry * 2 + 1) % len(CAN_COLORS)])
    rect(d, 0, 14, 16, 2, (48, 58, 50, 255))
    px(d, 2, 15, GREY_XXD); px(d, 13, 15, GREY_XXD)
    return im

def _binarize_alpha(img):
    """Snap the alpha channel to 0/255 so the sheets keep binary alpha
    even where an Antea sprite carries soft translucent edge pixels."""
    a = img.getchannel("A").point(lambda v: 255 if v >= 128 else 0)
    img.putalpha(a)
    return img

def main():
    outs = {
        "office-tileset.png": build_office(),
        "a5-office-floors-walls.png": build_a5(),
        "interiors.png": build_interiors(),
    }
    for name, img in outs.items():
        img = _binarize_alpha(img)
        path = os.path.join(REPO_TILESETS, name)
        img.save(path)
        print("wrote", path, img.size)
        # also drop a copy next to this script for review
        img.save(os.path.join(os.path.dirname(os.path.abspath(__file__)),
                              "gen-" + name))

if __name__ == "__main__":
    main()

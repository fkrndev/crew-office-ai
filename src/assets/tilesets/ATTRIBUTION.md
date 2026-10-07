# Asset attribution

## Tilesets — procedural base + Antea furniture (CC-BY)

The tileset PNGs in `tilesets/` are produced by `scripts/gen_tilesets.py`. The
**floor, wall and carpet tiles are the project's own procedural art** (no third-party
license). The **furniture sprites** (desks, chairs, cabinets, vending machine,
bookshelf, plants, wall props) are adapted from
**[Furniture Office Equipment Set](https://stcrbcn.itch.io/furniture-office-set)
by Antea ✮⋆˙**, used under **CC-BY** — commercial use allowed, attribution
required. Credit line: *"Office furniture: Antea (CC-BY)."*

The original LimeZu tilesets were replaced on 2026-09-08 because the purchased
Complete Version licence did not transfer cleanly to this project's owner. The
replacement keeps the exact same gid layout (same image dimensions, same 16×16
grid, sprites painted only at the slots the maps use) so `maps/office.tmj` and
`maps/brooklyn99.tmj` render correctly unmodified.

A zip backup of the LimeZu originals is kept OUTSIDE the repo (not
redistributed) at `D:\project\super-agent\limezu-assets-backup-2026.zip`.

## Tiled maps

`maps/office.tmj` and `maps/brooklyn99.tmj` are original Tiled JSON layouts
hand-authored for this project (adapted from the map structure of
[`shahar061/the-office`](https://github.com/shahar061/the-office), project code:
ISC). They contain tile-id references and object coordinates only — no image
data — and are not derived LimeZu art.

## Cast

Every character — card portraits and walking sprites on the office floor — is
drawn procedurally in
[`scene/office/portraitArt.ts`](../scene/office/portraitArt.ts) from
per-character recipes. Original work, no third-party licence.

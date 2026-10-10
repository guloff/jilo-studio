# Jilo Studio icon

The icon is an aperture of six curved blades in a violet gradient (`#E3C8FF` → `#7B2FF7`) on a
dark plate (`#15131D`), on Apple's macOS icon grid (an 824 × 824 plate with 100 px margins on a
1024 canvas). Its concept was generated with Higgsfield (GPT Image 2.5) at the owner's request;
these scripts draw it as exact geometry, so every size is sharp. The files it makes are the
owner's marks (`assets/brand/jilo/LICENSE-brand.txt`).

Each blade edge is an arc of radius `rho` tangent to an inner circle of radius `r`; neighbouring
edges cross at the corners of the opening, and each edge runs from its corner out past the rim. The
disc is filled with the gradient and the edges and the opening are cut out to the plate. Colours are
parameters of `geom.py` (`plate=`, `c1=`, `c2=`, `paper=` for the DMG's light side).

| Script | Makes |
|---|---|
| `geom.py` | `mark.json` (the geometry) and `jilo-studio.svg` (the canonical vector) |
| `draw.swift` | a PNG of the icon at any size, with the plate on the grid, without it, or cropped to the plate (CoreGraphics, no extra tools) |
| `bg.py`, `bg.swift` | the DMG background: `background.svg` and its 1x / 2x renders |

## Regenerate (macOS)

```sh
cd packaging/macos/icon
python3 geom.py rho=420 r=70 gap=22
mkdir -p Jilo.iconset
for s in 16 32 128 256 512; do
  swift draw.swift mark.json Jilo.iconset/icon_${s}x${s}.png $s
  swift draw.swift mark.json Jilo.iconset/icon_${s}x${s}@2x.png $((s*2))
done
iconutil -c icns Jilo.iconset -o ../../../assets/brand/jilo/jilo-studio.icns
swift draw.swift mark.json ../../../assets/brand/jilo/jilo-studio-512.png 512
swift draw.swift mark.json ../../../assets/brand/jilo/jilo-studio-mark-256.png 256 crop
cp jilo-studio.svg ../../../assets/brand/jilo/
python3 bg.py && swift bg.swift bg.json bg1.png 1 && swift bg.swift bg.json bg2.png 2
sips -s format tiff -s dpiWidth 72 -s dpiHeight 72 bg1.png --out bg1.tiff
sips -s format tiff -s dpiWidth 144 -s dpiHeight 144 bg2.png --out bg2.tiff
tiffutil -cathidpicheck bg1.tiff bg2.tiff -out ../dmg/background.tiff
cp background.svg ../dmg/
rm -rf Jilo.iconset mark.json bg.json jilo-studio.svg background.svg bg1.* bg2.*
```

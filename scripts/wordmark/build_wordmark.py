"""Generate frontend/src/icons/wordmark.svg, the "mAIestro Code" wordmark.

The wordmark is drawn as outlines rather than live text so it renders the same
on macOS and Windows. It is set in Inter Bold with Inter's serifed capital I
(character variant cv08), so the I in "mAIestro" can't be read as a lowercase l.

Usage:
    pip install uharfbuzz fonttools
    python scripts/wordmark/build_wordmark.py <path/to/Inter-Bold.ttf> frontend/src/icons/wordmark.svg

Inter-Bold.ttf is `extras/ttf/Inter-Bold.ttf` in the Inter 4.1 release zip
(https://github.com/rsms/inter/releases/tag/v4.1). Inter is under the SIL Open
Font License; see Inter-OFL.txt next to this script.

The letters fill with `currentColor` and the AI with `var(--wordmark-accent)`,
so the page's CSS colors it per theme. The viewBox is the ink bounds, 100 units
of cap height plus the round letters' overshoot, so a CSS height of
0.7275 × font-size × (viewBox height / 100) matches Inter text at that size.
"""
import sys

import uharfbuzz as hb
from fontTools.pens.boundsPen import BoundsPen
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont

CAP = 100.0  # cap height in SVG units

# (text, HarfBuzz features, which path it goes in)
RUNS = [
    ("m", {}, "base"),
    ("AI", {"cv08": True}, "ai"),
    ("estro Code", {}, "base"),
]


def num(v):
    return f"{v:.2f}".rstrip("0").rstrip(".")


def main(font_path, out_path):
    tt = TTFont(font_path)
    glyphset = tt.getGlyphSet()
    order = tt.getGlyphOrder()
    font = hb.Font(hb.Face(hb.Blob.from_file_path(font_path)))
    scale = CAP / tt["OS/2"].sCapHeight

    x = 0.0
    paths = {"base": [], "ai": []}
    bounds = None
    for text, features, group in RUNS:
        buf = hb.Buffer()
        buf.add_str(text)
        buf.guess_segment_properties()
        hb.shape(font, buf, features)
        for info, pos in zip(buf.glyph_infos, buf.glyph_positions):
            glyph = glyphset[order[info.codepoint]]
            # Font units are y-up and SVG is y-down; the baseline is y=0.
            t = (scale, 0, 0, -scale, x + pos.x_offset * scale, -pos.y_offset * scale)
            pen = SVGPathPen(glyphset, ntos=num)
            glyph.draw(TransformPen(pen, t))
            bp = BoundsPen(glyphset)
            glyph.draw(TransformPen(bp, t))
            if bp.bounds:
                b = bp.bounds
                bounds = b if bounds is None else (
                    min(bounds[0], b[0]), min(bounds[1], b[1]),
                    max(bounds[2], b[2]), max(bounds[3], b[3]))
            if pen.getCommands():
                paths[group].append(pen.getCommands())
            x += pos.x_advance * scale

    minx, miny, maxx, maxy = bounds
    vb = " ".join(num(v) for v in (minx, miny, maxx - minx, maxy - miny))
    svg = (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{vb}" role="img" aria-label="mAIestro Code">\n'
        f'  <path fill="currentColor" d="{"".join(paths["base"])}"/>\n'
        f'  <path style="fill: var(--wordmark-accent, #facc15)" d="{"".join(paths["ai"])}"/>\n'
        f"</svg>\n"
    )
    with open(out_path, "w") as f:
        f.write(svg)


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])

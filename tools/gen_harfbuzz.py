#!/usr/bin/env python3
"""Shape the words of tests/data/arabic-words.txt with HarfBuzz and write
the glyphs and positions that tests/arabic.rs expects sundowner to produce,
to tests/data/arabic-harfbuzz.txt.

Each word is shaped with the four styles of Amiri, as one run: right to
left, or left to right for a word of digits (which the bidirectional
algorithm lays out left to right). HarfBuzz is used at its defaults, so
positions are in font units, unhinted (device tables play no part).
Default ignorables (the zero width non-joiner) are left out, as sundowner
draws no glyph for them.

Usage: pip install uharfbuzz==0.56.2 && python3 tools/gen_harfbuzz.py
"""

import os
import re

import uharfbuzz as hb

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
STYLES = ["Regular", "Bold", "Italic", "BoldItalic"]


def main():
    with open(os.path.join(ROOT, "tests", "data", "arabic-words.txt"), encoding="utf-8") as f:
        words = [w for w in f.read().split("\n") if w and not w.startswith("#")]
    out = [f"# HarfBuzz {hb.version_string()}: style, word, then glyph:advance:x_offset:y_offset in logical order"]
    for style in STYLES:
        path = os.path.join(ROOT, "fonts", "silk", f"Amiri-{style}.ttf")
        with open(path, "rb") as f:
            face = hb.Face(f.read())
        font = hb.Font(face)
        space = font.get_nominal_glyph(ord(" "))
        for word in words:
            buf = hb.Buffer()
            buf.add_str(word)
            buf.guess_segment_properties()
            ltr = re.fullmatch("[٠-٩]+", word) is not None
            buf.direction = "ltr" if ltr else "rtl"
            hb.shape(font, buf)
            glyphs = [
                (i.codepoint, p.x_advance, p.x_offset, p.y_offset)
                for i, p in zip(buf.glyph_infos, buf.glyph_positions)
                if not (i.codepoint == space and p.x_advance == 0)
            ]
            if not ltr:
                glyphs.reverse()  # HarfBuzz gives visual order
            out.append("\t".join([style, word] + [":".join(map(str, g)) for g in glyphs]))
    with open(os.path.join(ROOT, "tests", "data", "arabic-harfbuzz.txt"), "w", encoding="utf-8") as f:
        f.write("\n".join(out) + "\n")
    print(f"{len(words)} words, {len(words) * len(STYLES)} cases")


if __name__ == "__main__":
    main()

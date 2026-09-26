#!/usr/bin/env python3
"""Regenerate the fonts bundled into the sundowner binary.

Sources are pinned to a google/fonts commit. Fonts published only as
variable fonts are instantiated with fontTools at Regular (wght=400) and
Bold (wght=700), and at their normal width where they have a width axis.
None of their licenses declares a Reserved Font Name, so these Modified
Versions may keep their names; they remain under the OFL. Fonts published
as static files are copied byte-for-byte.

The fonts directly in fonts/ are the base set, built into every binary.
fonts/silk/ holds the fonts of the `silk` tier (Hebrew, Armenian, Chinese,
Japanese and Korean), built in with `cargo build --features silk`.

Usage: pip install fonttools==4.66.0 && python3 fonts/build.py
"""
import hashlib
import io
import pathlib
import urllib.request

from fontTools.ttLib import TTFont
from fontTools.varLib import instancer

COMMIT = "23e54b51ddffbc7713c583748e3bd86f62b1fa4a"
BASE = f"https://raw.githubusercontent.com/google/fonts/{COMMIT}/ofl"
OUT = pathlib.Path(__file__).resolve().parent
SILK = OUT / "silk"


def fetch(path):
    with urllib.request.urlopen(f"{BASE}/{path}") as r:
        return r.read()


def instance(src, weight, dest, width=None):
    # Keep the source timestamp so rebuilding gives identical files.
    font = TTFont(io.BytesIO(fetch(src)), recalcTimestamp=False)
    axes = {"wght": weight}
    if width is not None:
        axes["wdth"] = width
    static = instancer.instantiateVariableFont(font, axes, updateFontNames=True)
    static.recalcTimestamp = False
    static.save(dest)


# Base set: Latin, Greek and Cyrillic.
instance("alegreya/Alegreya%5Bwght%5D.ttf", 400, OUT / "Alegreya-Regular.ttf")
instance("alegreya/Alegreya%5Bwght%5D.ttf", 700, OUT / "Alegreya-Bold.ttf")
instance("alegreya/Alegreya-Italic%5Bwght%5D.ttf", 400, OUT / "Alegreya-Italic.ttf")
instance("alegreya/Alegreya-Italic%5Bwght%5D.ttf", 700, OUT / "Alegreya-BoldItalic.ttf")
(OUT / "OFL-Alegreya.txt").write_bytes(fetch("alegreya/OFL.txt"))

for style in ["Regular", "Bold", "Italic", "BoldItalic"]:
    (OUT / f"Cousine-{style}.ttf").write_bytes(fetch(f"cousine/Cousine-{style}.ttf"))
(OUT / "OFL-Cousine.txt").write_bytes(fetch("cousine/OFL.txt"))

# Silk tier: Hebrew, Armenian, Chinese, Japanese and Korean.
SILK.mkdir(exist_ok=True)
for family, src, width in [
    ("FrankRuhlLibre", "frankruhllibre/FrankRuhlLibre%5Bwght%5D.ttf", None),
    ("NotoSerifHebrew", "notoserifhebrew/NotoSerifHebrew%5Bwdth,wght%5D.ttf", 100),
    ("NotoSerifArmenian", "notoserifarmenian/NotoSerifArmenian%5Bwdth,wght%5D.ttf", 100),
    ("NotoSerifSC", "notoserifsc/NotoSerifSC%5Bwght%5D.ttf", None),
]:
    for style, weight in [("Regular", 400), ("Bold", 700)]:
        instance(src, weight, SILK / f"{family}-{style}.ttf", width)
    (SILK / f"OFL-{family}.txt").write_bytes(fetch(f"{src.split('/')[0]}/OFL.txt"))

for style in ["Regular", "Bold"]:
    (SILK / f"GowunBatang-{style}.ttf").write_bytes(fetch(f"gowunbatang/GowunBatang-{style}.ttf"))
(SILK / "OFL-GowunBatang.txt").write_bytes(fetch("gowunbatang/OFL.txt"))

for f in sorted(OUT.glob("*.ttf")) + sorted(SILK.glob("*.ttf")):
    print(hashlib.sha256(f.read_bytes()).hexdigest(), f.relative_to(OUT))

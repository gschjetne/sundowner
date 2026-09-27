#!/usr/bin/env python3
"""Regenerate the fonts bundled into the sundowner binary.

Sources are pinned to a google/fonts commit. Fonts published only as
variable fonts are instantiated with fontTools at Regular (wght=400) and
Bold (wght=700), and at their normal width where they have a width axis.
None of their licenses declares a Reserved Font Name, so these Modified
Versions may keep their names; they remain under the OFL. Fonts published
as static files are copied byte-for-byte. Noto Serif JP is also cut down
to the Japanese kanji that Noto Serif TC lacks.

The fonts directly in fonts/ are the base set, built into every binary.
fonts/silk/ holds the fonts of the `silk` tier (Hebrew, Arabic, Armenian,
Chinese, Japanese and Korean), built in with `cargo build --features silk`.

Usage: pip install fonttools==4.66.0 && python3 fonts/build.py
"""
import hashlib
import io
import pathlib
import urllib.request

from fontTools import subset
from fontTools.ttLib import TTFont
from fontTools.varLib import instancer

COMMIT = "23e54b51ddffbc7713c583748e3bd86f62b1fa4a"
BASE = f"https://raw.githubusercontent.com/google/fonts/{COMMIT}/ofl"
OUT = pathlib.Path(__file__).resolve().parent
SILK = OUT / "silk"


def fetch(path):
    with urllib.request.urlopen(f"{BASE}/{path}") as r:
        return r.read()


def instance(src, weight, dest=None, width=None):
    # Keep the source timestamp so rebuilding gives identical files, and
    # always use fontTools' own table packer: with uharfbuzz installed it
    # would otherwise use HarfBuzz's, which lays out GSUB and GPOS
    # differently.
    font = TTFont(
        io.BytesIO(fetch(src)),
        recalcTimestamp=False,
        cfg={"fontTools.ttLib.tables.otBase:USE_HARFBUZZ_REPACKER": False},
    )
    axes = {"wght": weight}
    if width is not None:
        axes["wdth"] = width
    static = instancer.instantiateVariableFont(font, axes, updateFontNames=True)
    static.recalcTimestamp = False
    if dest is None:
        return static
    static.save(dest)


# CJK Unified Ideographs, Extension A, the compatibility ideographs and
# Extensions B and later.
HAN = [range(0x3400, 0x4DC0), range(0x4E00, 0xA000), range(0xF900, 0xFB00), range(0x20000, 0x40000)]


def jis_kanji():
    """The kanji of JIS X 0208 and of the first plane of JIS X 0213, which
    include every kanji in common Japanese use (the Joyo and Jinmeiyo kanji
    among them)."""
    kanji = set()
    rows = range(0xA1, 0xFF)
    for a in rows:
        for b in rows:
            try:
                s = bytes([a, b]).decode("euc_jis_2004")
            except UnicodeDecodeError:
                continue
            if len(s) == 1 and any(ord(s) in r for r in HAN):
                kanji.add(ord(s))
    return kanji


# Base set: Latin, Greek and Cyrillic.
instance("alegreya/Alegreya%5Bwght%5D.ttf", 400, OUT / "Alegreya-Regular.ttf")
instance("alegreya/Alegreya%5Bwght%5D.ttf", 700, OUT / "Alegreya-Bold.ttf")
instance("alegreya/Alegreya-Italic%5Bwght%5D.ttf", 400, OUT / "Alegreya-Italic.ttf")
instance("alegreya/Alegreya-Italic%5Bwght%5D.ttf", 700, OUT / "Alegreya-BoldItalic.ttf")
(OUT / "OFL-Alegreya.txt").write_bytes(fetch("alegreya/OFL.txt"))

for style in ["Regular", "Bold", "Italic", "BoldItalic"]:
    (OUT / f"Cousine-{style}.ttf").write_bytes(fetch(f"cousine/Cousine-{style}.ttf"))
(OUT / "OFL-Cousine.txt").write_bytes(fetch("cousine/OFL.txt"))

# Silk tier: Hebrew, Arabic, Armenian, Chinese, Japanese and Korean.
SILK.mkdir(exist_ok=True)
for family, src, width in [
    ("FrankRuhlLibre", "frankruhllibre/FrankRuhlLibre%5Bwght%5D.ttf", None),
    ("NotoSerifHebrew", "notoserifhebrew/NotoSerifHebrew%5Bwdth,wght%5D.ttf", 100),
    ("NotoSerifArmenian", "notoserifarmenian/NotoSerifArmenian%5Bwdth,wght%5D.ttf", 100),
    ("NotoSerifTC", "notoseriftc/NotoSerifTC%5Bwght%5D.ttf", None),
]:
    for style, weight in [("Regular", 400), ("Bold", 700)]:
        instance(src, weight, SILK / f"{family}-{style}.ttf", width)
    (SILK / f"OFL-{family}.txt").write_bytes(fetch(f"{src.split('/')[0]}/OFL.txt"))

# Noto Serif JP, cut down to the Japanese kanji that Noto Serif TC lacks:
# those simplified in Japan after 1946 (such as 気, 楽 and 読) and a few
# others, so that Japanese text is complete without a second full CJK font.
for style, weight in [("Regular", 400), ("Bold", 700)]:
    traditional = TTFont(SILK / f"NotoSerifTC-{style}.ttf").getBestCmap()
    font = instance("notoserifjp/NotoSerifJP%5Bwght%5D.ttf", weight)
    options = subset.Options()
    options.name_IDs = ["*"]
    options.name_languages = ["*"]
    options.notdef_outline = True
    options.recalc_timestamp = False
    subsetter = subset.Subsetter(options)
    subsetter.populate(unicodes=sorted(jis_kanji() - set(traditional)))
    subsetter.subset(font)
    font.save(SILK / f"NotoSerifJP-{style}.ttf")
(SILK / "OFL-NotoSerifJP.txt").write_bytes(fetch("notoserifjp/OFL.txt"))

for style in ["Regular", "Bold"]:
    (SILK / f"GowunBatang-{style}.ttf").write_bytes(fetch(f"gowunbatang/GowunBatang-{style}.ttf"))
(SILK / "OFL-GowunBatang.txt").write_bytes(fetch("gowunbatang/OFL.txt"))

for style in ["Regular", "Bold", "Italic", "BoldItalic"]:
    (SILK / f"Amiri-{style}.ttf").write_bytes(fetch(f"amiri/Amiri-{style}.ttf"))
(SILK / "OFL-Amiri.txt").write_bytes(fetch("amiri/OFL.txt"))

for f in sorted(OUT.glob("*.ttf")) + sorted(SILK.glob("*.ttf")):
    print(hashlib.sha256(f.read_bytes()).hexdigest(), f.relative_to(OUT))

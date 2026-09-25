#!/usr/bin/env python3
"""Regenerate the fonts bundled into the sundowner binary.

Sources are pinned to a google/fonts commit. Alegreya and Roboto Mono are
published only as variable fonts, so static Regular (wght=400) and Bold
(wght=700) instances of the upright and italic fonts are generated with
fontTools. Neither font's OFL declares a Reserved Font Name, so these
Modified Versions may keep their names; they remain under the OFL (see
OFL-Alegreya.txt and OFL-RobotoMono.txt).

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


def fetch(path):
    with urllib.request.urlopen(f"{BASE}/{path}") as r:
        return r.read()


def instance(src, weight, dest):
    # Keep the source timestamp so rebuilding gives identical files.
    font = TTFont(io.BytesIO(fetch(src)), recalcTimestamp=False)
    static = instancer.instantiateVariableFont(font, {"wght": weight}, updateFontNames=True)
    static.recalcTimestamp = False
    static.save(OUT / dest)


instance("alegreya/Alegreya%5Bwght%5D.ttf", 400, "Alegreya-Regular.ttf")
instance("alegreya/Alegreya%5Bwght%5D.ttf", 700, "Alegreya-Bold.ttf")
instance("alegreya/Alegreya-Italic%5Bwght%5D.ttf", 400, "Alegreya-Italic.ttf")
instance("alegreya/Alegreya-Italic%5Bwght%5D.ttf", 700, "Alegreya-BoldItalic.ttf")
(OUT / "OFL-Alegreya.txt").write_bytes(fetch("alegreya/OFL.txt"))

instance("robotomono/RobotoMono%5Bwght%5D.ttf", 400, "RobotoMono-Regular.ttf")
instance("robotomono/RobotoMono%5Bwght%5D.ttf", 700, "RobotoMono-Bold.ttf")
instance("robotomono/RobotoMono-Italic%5Bwght%5D.ttf", 400, "RobotoMono-Italic.ttf")
instance("robotomono/RobotoMono-Italic%5Bwght%5D.ttf", 700, "RobotoMono-BoldItalic.ttf")
(OUT / "OFL-RobotoMono.txt").write_bytes(fetch("robotomono/OFL.txt"))

for f in sorted(OUT.glob("*.ttf")):
    print(hashlib.sha256(f.read_bytes()).hexdigest(), f.name)

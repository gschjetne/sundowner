# Bundled fonts

These fonts are compiled into the sundowner binary and embedded as subsets
in the PDFs it creates. The fonts directly in this directory are in every
build; those in `silk/` only in the silk build (`--features silk`), which
stores them compressed.

| File | Font | License | Changes |
|------|------|---------|---------|
| `Alegreya-Regular.ttf`, `Alegreya-Bold.ttf`, `Alegreya-Italic.ttf`, `Alegreya-BoldItalic.ttf` | [Alegreya](https://github.com/huertatipografica/Alegreya) by Huerta Tipográfica | SIL OFL 1.1, [`OFL-Alegreya.txt`](OFL-Alegreya.txt) | Static instances (`wght` 400 and 700) generated from the upstream variable fonts. No other changes. |
| `Cousine-Regular.ttf`, `Cousine-Bold.ttf`, `Cousine-Italic.ttf`, `Cousine-BoldItalic.ttf` | [Cousine](https://github.com/googlefonts/cousine) by The Cousine Project Authors | SIL OFL 1.1, [`OFL-Cousine.txt`](OFL-Cousine.txt) | None: the files are byte-for-byte upstream. |

| `silk/FrankRuhlLibre-Regular.ttf`, `silk/FrankRuhlLibre-Bold.ttf` | [Frank Ruhl Libre](https://github.com/fontef/frankruhllibre) by Yanek Iontef | SIL OFL 1.1, [`silk/OFL-FrankRuhlLibre.txt`](silk/OFL-FrankRuhlLibre.txt) | Static instances (`wght` 400 and 700) generated from the upstream variable font. No other changes. |
| `silk/NotoSerifHebrew-Regular.ttf`, `silk/NotoSerifHebrew-Bold.ttf` | [Noto Serif Hebrew](https://github.com/notofonts/hebrew) by The Noto Project Authors | SIL OFL 1.1, [`silk/OFL-NotoSerifHebrew.txt`](silk/OFL-NotoSerifHebrew.txt) | Static instances (`wght` 400 and 700, `wdth` 100) generated from the upstream variable font. No other changes. |
| `silk/NotoSerifArmenian-Regular.ttf`, `silk/NotoSerifArmenian-Bold.ttf` | [Noto Serif Armenian](https://github.com/notofonts/armenian) by The Noto Project Authors | SIL OFL 1.1, [`silk/OFL-NotoSerifArmenian.txt`](silk/OFL-NotoSerifArmenian.txt) | Static instances (`wght` 400 and 700, `wdth` 100) generated from the upstream variable font. No other changes. |
| `silk/NotoSerifSC-Regular.ttf`, `silk/NotoSerifSC-Bold.ttf` | [Noto Serif SC](https://github.com/notofonts/noto-cjk) by Google | SIL OFL 1.1, [`silk/OFL-NotoSerifSC.txt`](silk/OFL-NotoSerifSC.txt) | Static instances (`wght` 400 and 700) generated from the upstream variable font. No other changes. |
| `silk/GowunBatang-Regular.ttf`, `silk/GowunBatang-Bold.ttf` | [Gowun Batang](https://github.com/yangheeryu/Gowun-Batang) by Yanghee Ryu | SIL OFL 1.1, [`silk/OFL-GowunBatang.txt`](silk/OFL-GowunBatang.txt) | None: the files are byte-for-byte upstream. |

Alegreya and Cousine cover Latin (Basic Latin, Latin-1 and Latin Extended-A),
modern and polytonic Greek, and Cyrillic. The silk fonts add Hebrew
(Frank Ruhl Libre, with Noto Serif Hebrew for cantillation marks),
Armenian, Chinese characters and Japanese kana (Noto Serif SC), and Korean
Hangul (Gowun Batang).

`build.py` regenerates every file here from a pinned
[google/fonts](https://github.com/google/fonts) commit. The build is
reproducible: the output must match [`SHA256SUMS`](SHA256SUMS)
(`cd fonts && sha256sum -c SHA256SUMS`).

## How the fonts comply with the SIL Open Font License

- **License text travels with the fonts.** The copyright notices and license
  are kept here in the source tree. The binary embeds them too, and
  `sundowner --licenses` prints them (OFL condition 2).
- **Reserved Font Names are respected.** The licenses of Alegreya, Frank
  Ruhl Libre, Noto Serif Hebrew, Noto Serif Armenian and Noto Serif SC
  declare no Reserved Font Name, so the generated static instances, which
  are Modified Versions, may keep their names (OFL condition 3). The
  Cousine and Gowun Batang files are not modified.
- **Modified Versions stay under the OFL.** The static instances remain
  under the OFL (condition 5). They are not sold by themselves; they are
  bundled with software (condition 1).
- **Embedding in PDFs is allowed.** sundowner embeds subsets of these fonts
  in the PDFs it generates. The OFL permits embedding fonts in documents,
  and it does not place the documents under the OFL.

## Changes (FONTLOG)

- 2026-09-25: Added Alegreya Regular, Bold, Italic and Bold Italic, generated
  with fontTools 4.66.0 `instancer` (`updateFontNames=True`) from
  `ofl/alegreya/Alegreya[wght].ttf` and `Alegreya-Italic[wght].ttf` at
  google/fonts `23e54b51ddffbc7713c583748e3bd86f62b1fa4a`. Added IBM Plex
  Mono Regular and Bold, unmodified, from the same commit.
- 2026-09-25: Replaced IBM Plex Mono, which lacks Greek, with Roboto Mono
  Regular, Bold, Italic and Bold Italic, generated the same way from
  `ofl/robotomono/RobotoMono[wght].ttf` and `RobotoMono-Italic[wght].ttf` at
  the same commit. Made the build reproducible by keeping the source
  timestamps (`recalcTimestamp=False`), and added `SHA256SUMS`.
- 2026-09-26: Replaced Roboto Mono, which lacks polytonic Greek, with
  Cousine Regular, Bold, Italic and Bold Italic, unmodified, from
  `ofl/cousine/` at the same commit.
- 2026-09-26: Added the silk tier in `silk/`: Frank Ruhl Libre, Noto Serif
  Hebrew, Noto Serif Armenian and Noto Serif SC Regular and Bold, generated
  with fontTools 4.66.0 `instancer` from `ofl/frankruhllibre/`,
  `ofl/notoserifhebrew/`, `ofl/notoserifarmenian/` and `ofl/notoserifsc/`
  at the same commit (with `wdth` 100 where the font has a width axis), and
  Gowun Batang Regular and Bold, unmodified, from `ofl/gowunbatang/`.

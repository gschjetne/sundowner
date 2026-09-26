# Bundled fonts

These fonts are compiled into the sundowner binary and embedded as subsets
in the PDFs it creates.

| File | Font | License | Changes |
|------|------|---------|---------|
| `Alegreya-Regular.ttf`, `Alegreya-Bold.ttf`, `Alegreya-Italic.ttf`, `Alegreya-BoldItalic.ttf` | [Alegreya](https://github.com/huertatipografica/Alegreya) by Huerta Tipográfica | SIL OFL 1.1, [`OFL-Alegreya.txt`](OFL-Alegreya.txt) | Static instances (`wght` 400 and 700) generated from the upstream variable fonts. No other changes. |
| `Cousine-Regular.ttf`, `Cousine-Bold.ttf`, `Cousine-Italic.ttf`, `Cousine-BoldItalic.ttf` | [Cousine](https://github.com/googlefonts/cousine) by The Cousine Project Authors | SIL OFL 1.1, [`OFL-Cousine.txt`](OFL-Cousine.txt) | None: the files are byte-for-byte upstream. |

All eight fonts cover Latin (Basic Latin, Latin-1 and Latin Extended-A),
modern and polytonic Greek, and Cyrillic.

`build.py` regenerates every file here from a pinned
[google/fonts](https://github.com/google/fonts) commit. The build is
reproducible: the output must match [`SHA256SUMS`](SHA256SUMS)
(`cd fonts && sha256sum -c SHA256SUMS`).

## How the fonts comply with the SIL Open Font License

- **License text travels with the fonts.** The copyright notices and license
  are kept here in the source tree. The binary embeds them too, and
  `sundowner --licenses` prints them (OFL condition 2).
- **Reserved Font Names are respected.** Alegreya's license declares no
  Reserved Font Name, so the generated static instances, which are Modified
  Versions, may keep the name (OFL condition 3). The Cousine files are not
  modified.
- **Modified Versions stay under the OFL.** The Alegreya instances remain
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

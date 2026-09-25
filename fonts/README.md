# Bundled fonts

These fonts are compiled into the sundowner binary and embedded as subsets
in the PDFs it creates.

| File | Font | License | Changes |
|------|------|---------|---------|
| `Alegreya-Regular.ttf`, `Alegreya-Bold.ttf`, `Alegreya-Italic.ttf`, `Alegreya-BoldItalic.ttf` | [Alegreya](https://github.com/huertatipografica/Alegreya) by Huerta Tipográfica | SIL OFL 1.1, [`OFL-Alegreya.txt`](OFL-Alegreya.txt) | Static instances (`wght` 400 and 700) generated from the upstream variable fonts. No other changes. |
| `IBMPlexMono-Regular.ttf`, `IBMPlexMono-Bold.ttf` | [IBM Plex Mono](https://github.com/IBM/plex) by IBM | SIL OFL 1.1, [`OFL-IBMPlexMono.txt`](OFL-IBMPlexMono.txt) | None: the files are byte-for-byte upstream. |

`build.py` regenerates every file here from a pinned
[google/fonts](https://github.com/google/fonts) commit. It prints each
file's SHA-256 checksum.

## How the fonts comply with the SIL Open Font License

- **License text travels with the fonts.** The copyright notices and license
  are kept here in the source tree. The binary embeds them too, and
  `sundowner --licenses` prints them (OFL condition 2).
- **Reserved Font Names are respected.** Alegreya's license declares no
  Reserved Font Name, so the generated static instances, which are Modified
  Versions, may keep the name. IBM Plex Mono reserves "Plex", so its files
  are shipped unmodified (OFL condition 3).
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

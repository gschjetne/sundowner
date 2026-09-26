# sundowner

Markdown to PDF converter. A single static binary with no runtime or crate
dependencies. It is written in safe Rust (`#![forbid(unsafe_code)]`) and uses
only the standard library.

Output is predictable: text is set in the bundled
[Alegreya](https://github.com/huertatipografica/Alegreya) and code in
[Cousine](https://github.com/googlefonts/cousine), both embedded in every
PDF.
Fonts installed on the machine are never used, so the same input and
configuration produce the same PDF everywhere.

```sh
sundowner notes.md                 # writes notes.pdf
sundowner *.md                     # one PDF per input
sundowner -o out.pdf < in.md       # stdin -> file
cat in.md | sundowner > out.pdf    # stdin -> stdout
```

## Options

```
-o, --output <FILE>     Output file ('-' for standard output); single input only
-c, --config <FILE>     Settings and fonts file (default: the nearest .sundowner
                        in the input's directory or a parent directory)
    --no-config         Ignore .sundowner files
-p, --paper <SIZE>      a4 (default), a3, a5, letter, legal, or WIDTHxHEIGHT in mm
-s, --font-size <PT>    Body font size in points (default 11)
-m, --margin <MM>       Page margin in millimetres (default 20)
-t, --title <TEXT>      Document title (default: first level-1 heading)
    --no-page-numbers   Do not number pages
    --no-images         Do not load image files
-q, --quiet             Do not print warnings
    --licenses          Show the licenses of sundowner and its bundled fonts
```

Command-line options override the configuration file.

Exit status: `0` on success, `1` if any input failed, `2` on usage errors.

## Fonts and configuration

Every bundled font, in every style, covers Latin (including Central and
Eastern European), modern and polytonic Greek, and Cyrillic; a test enforces
this. To use other fonts, or to cover other scripts and emoji,
create a `.sundowner` file. sundowner uses the nearest one in the input
file's directory or any parent directory, or the file given with `--config`.
The search goes all the way up, so a `.sundowner` in your home directory
(or in `/`) applies to every document below it that has no closer one; use
`--no-config` to ignore configuration files.

```ini
# Page settings (same meaning as the command-line options)
paper = a4
font-size = 11
margin = 20
page-numbers = true

# Replace Alegreya for body text and headings, e.g. with a sans-serif
[body]
regular = fonts/SourceSans3-Regular.ttf
bold = fonts/SourceSans3-Bold.ttf
italic = fonts/SourceSans3-Italic.ttf
bold-italic = fonts/SourceSans3-BoldItalic.ttf

# Replace Cousine for code
[mono]
regular = fonts/JetBrainsMono-Regular.ttf
bold = fonts/JetBrainsMono-Bold.ttf

# Fonts for characters the fonts above lack. Repeat the section to add
# more; they are tried in order.
[fallback]
regular = fonts/NotoSansJP-Regular.ttf
bold = fonts/NotoSansJP-Bold.ttf

[fallback]
regular = fonts/NotoEmoji-Regular.ttf
```

- **Paths** are relative to the `.sundowner` file. Select a font inside a
  TrueType collection with `#index`, e.g. `fonts/NotoSansCJK.ttc#2`.
- **Character lookup.** For each character, sundowner uses the first font
  that has a glyph for it:
  - body text: `[body]`, then the `[fallback]` fonts, then the bundled fonts
  - code: `[mono]`, then the `[fallback]` fonts, then the body font, then
    the bundled fonts

  Characters that no font covers are drawn as the font's empty box, and a
  warning lists them.
- **Styles are never synthesized.** If a family has no bold or italic font,
  its regular font is used as-is. There is no fake bolding or slanting.
- **Font format.** Fonts must be TrueType outline fonts (`.ttf`, or `.ttc`
  collections). OpenType fonts with PostScript (CFF) outlines, which are
  usually `.otf`, are rejected with an error. So are color emoji fonts; use
  a monochrome one such as Noto Emoji. Fonts whose license forbids embedding
  are refused.
- **Variable fonts** can only be embedded at their default instance. A
  warning appears when that isn't the weight the slot calls for; use static
  font files for exact weights.
- **Subsetting.** Only the glyphs a document uses are embedded, so large
  fonts, such as CJK fonts, add little to the PDF.


## Supported Markdown

CommonMark core plus the common GitHub extensions:

- ATX and setext headings, paragraphs, hard and soft line breaks
- `*emphasis*`, `**strong**`, `~~strikethrough~~`, `` `code` ``
- Fenced and indented code blocks, block quotes (nested)
- Ordered, unordered and nested lists, task lists (`- [x]`)
- Tables with column alignment and wrapping cells
- Inline, reference and autolinks (`<https://…>` and bare URLs). Links to
  `#heading-slugs` jump within the document.
- Local PNG and JPEG images, given as relative paths inside the Markdown
  file's directory. Transparent PNGs keep their transparency.
- HTML comments are dropped, `<br>` becomes a line break, other inline tags
  are removed, and HTML entities are decoded.

The PDF also gets a bookmark outline built from the headings, clickable links,
page numbers and a document title. Only `http:`, `https:` and `mailto:` links
and `#anchor` links become clickable; other links are shown as plain text.

## Design

- **No dependencies.** Everything is in `src/`: the Markdown parser, layout
  engine, Unicode line breaker, TrueType parser and subsetter, OpenType
  glyph substitution, PDF writer, DEFLATE codec and PNG/JPEG readers. The bundled fonts are compiled into the binary.
- **Self-contained PDFs.**
  - Every font is embedded as a subset (Type 0 / CIDFontType2 with
    Identity-H encoding), so viewers never substitute fonts.
  - A ToUnicode map keeps copy-paste and search working.
  - A one-word document is about 3 KB.
  - Subset names are derived from their contents, so identical input gives
    byte-identical output.
- **Ligatures and contextual forms.** Glyph substitutions come from the
  font's OpenType GSUB table, with the features HarfBuzz applies by default
  to Latin, Greek and Cyrillic: `ccmp`, `locl`, `rlig`, `liga`, `clig`,
  `calt`, `rclt` and `rvrn`, from the language system of the text's script.
  All GSUB lookup types are supported, including chained contextual and
  reverse chained substitutions, along with the lookup flags that skip
  marks or ligatures. For the bundled fonts the result matches HarfBuzz
  glyph for glyph. Ligatures map back to their characters in the ToUnicode
  map, so copying `office` from the PDF gives `office`. A zero-width
  non-joiner (U+200C) prevents a ligature. Code gets the same treatment, so
  programming fonts with ligatures show them; Cousine has none.
- **Kerning.** Pairs of adjacent glyphs are kerned using the font's
  OpenType `kern` feature (GPOS pair adjustment). Fonts without GPOS
  kerning fall back to the legacy `kern` table. Kerning is included when
  measuring text, so line breaks account for it. It applies to text in one
  font, not across spaces or font changes. As the OpenType spec prescribes,
  adjustments from all of the font's kerning lookups add up.
- **Line breaking** follows the Unicode Line Breaking Algorithm
  ([UAX #14](https://www.unicode.org/reports/tr14/), Unicode 17.0) in full,
  and passes its official conformance test. Lines break at spaces, after
  hyphens and dashes, and between CJK characters. They do not break before
  closing punctuation (`word !`, `« quote »`), inside numbers such as
  `$12.50`, or at no-break spaces. Break opportunities are computed over
  the whole paragraph, across style changes. A soft hyphen (U+00AD) marks
  a possible break, and shows a hyphen only if the line breaks there. There
  is no automatic hyphenation.
- **Compact output.** Content streams are compressed with a small built-in
  DEFLATE encoder (LZ77 with short hash chains and the fixed Huffman code).
  This is simple and fast but compresses less than zlib or gzip at their
  default settings.
- **Crash-proof.**
  - No `unsafe` code.
  - Nesting depth is capped.
  - Every parser scan is bounded, so hostile input cannot hang the parser
    (the CommonMark delimiter algorithm keeps emphasis parsing linear).
  - Font files are parsed with bounds checks everywhere, so malformed fonts
    are rejected with an error.
  - Images are limited in file size (64 MiB each, 256 MiB total per
    document) and dimensions (20,000 px per side, 64 megapixels), for every
    format.
  - Image paths must be relative and resolve (after following symlinks)
    inside the Markdown file's directory, so a document cannot embed
    arbitrary files from the system.
  - Output is written to a temporary file and renamed into place, so a
    failed run never leaves a truncated PDF.
  - If an internal error happens anyway, only that file fails and the run
    continues.

  The test suite converts thousands of random and adversarial documents and
  checks the resulting PDF structure.
- **Fast.** About 550 KB of Markdown (600 pages) converts in about 0.25 s
  using about 20 MB of memory.

## Limitations

- **Shaping is for Latin, Greek and Cyrillic.** Ligatures and contextual
  forms are applied (see above), but there is no mark positioning, no
  right-to-left layout and no script-specific shaping. Latin, Greek,
  Cyrillic and CJK render correctly. Arabic, Hebrew and Indic scripts do
  not.
- **Combining marks** are drawn as separate glyphs. They are not composed
  into precomposed characters.
- Remote images, interlaced PNGs and formats other than PNG and JPEG are not
  embedded. They appear as an italic `[image: …]` placeholder with a warning.

## Building

```sh
# Fully static Linux binary (about 3.1 MB, most of it the bundled fonts)
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
# -> target/x86_64-unknown-linux-musl/release/sundowner

# Regular build for the host platform
cargo build --release

cargo test                                   # quick
cargo test --release -- --include-ignored    # plus the stress tests (as in CI)
```

The line breaking tables in `src/linebreak_table.rs` are generated from the
Unicode Character Database by `tools/gen_linebreak.py` (Python 3, standard
library only). It also writes the conformance test data in `tests/data/`.
To move to another Unicode version, change `VERSION` in the script and run
it.

Requires Rust 1.87 or newer (the code uses `u*::is_multiple_of`, stabilized
in 1.87). CI checks formatting, clippy, the full test suite, the static build
and the minimum Rust version.

## License

The code is licensed under the MIT License (see `LICENSE`). The bundled
fonts in `fonts/` are licensed under the SIL Open Font License 1.1. See
[`fonts/README.md`](fonts/README.md) for their sources, changes and license
compliance, or run `sundowner --licenses`. The line breaking tables and test
data are derived from the Unicode Character Database, under the Unicode
License v3 (see `LICENSE-UNICODE`).

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

sundowner comes in two builds, which differ only in the fonts they bundle:

| Build | Scripts | Size |
|-------|---------|------|
| **europa** (default) | Latin, Greek, Cyrillic: the alphabets on euro banknotes | 3.2 MB |
| **silk** (`--features silk`) | also Hebrew, Arabic, Armenian, Chinese, Japanese and Korean: the scripts along the Silk Road | 20 MB |

The silk build adds fonts in the same humanist, broad-nib spirit as
Alegreya, as far as each writing system has one:
[Frank Ruhl Libre](https://github.com/fontef/frankruhllibre) for Hebrew
(with [Noto Serif Hebrew](https://github.com/notofonts/hebrew) for
cantillation marks, which Frank Ruhl Libre lacks),
[Amiri](https://github.com/aliftype/amiri), a revival of the Naskh of the
Bulaq press, for Arabic (and Persian, Urdu and other languages written in
Arabic script),
[Noto Serif Armenian](https://github.com/notofonts/armenian),
[Noto Serif TC](https://github.com/notofonts/noto-cjk), a Song/Mincho
face, for Chinese characters (in their traditional forms) and Japanese
kana, with the Japanese kanji it lacks from Noto Serif JP, and
[Gowun Batang](https://github.com/yangheeryu/Gowun-Batang), a
brush-inspired Batang, for Korean. Amiri has regular, bold, italic and
bold italic; the others have regular and bold, and italic text uses the
upright font, as these scripts have no italics. The
silk fonts are stored compressed and only unpacked when a document uses
them, so documents in the europa scripts are as fast, and come out the
same, with either build.

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
    --front-matter      Show YAML front matter as a table at the start
    --no-front-matter   Leave YAML front matter out (the default, but
                        without a warning that it is left out)
    --justify           Justify paragraphs (the default; overrides justify = false)
    --no-justify        Set paragraphs ragged (flush left) instead of justified
-l, --lang <TAG>        Language of the text, such as en or en-US (default: the
                        front matter's lang); English text is hyphenated
    --no-images         Do not load image files
-q, --quiet             Do not print warnings
    --licenses          Show the licenses of sundowner and its bundled fonts
```

Command-line options override the configuration file.

Exit status: `0` on success, `1` if any input failed, `2` on usage errors.

## Fonts and configuration

Alegreya and Cousine, in every style, cover Latin (including Central and
Eastern European), modern and polytonic Greek, and Cyrillic; a test enforces
this, and one that every script of the silk build is set in its font in
every style. To use other fonts, or to cover other scripts and emoji,
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
front-matter = false
justify = true
# Language of documents whose front matter does not give one. Every such
# document under this file is then hyphenated as that language, so set it
# only where all of them are, e.g. in the folder of an English book.
# lang = en-US

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
  - body text: `[body]`, then the `[fallback]` fonts, then the bundled fonts:
    Alegreya, (silk: Frank Ruhl Libre, Noto Serif Hebrew, Noto Serif
    Armenian), Cousine, (silk: Amiri, Noto Serif TC, Noto Serif JP,
    Gowun Batang)
  - code: `[mono]`, then the `[fallback]` fonts, then the body font, then
    the bundled fonts

  Characters that all scripts share, such as punctuation and digits, come
  from the font of the Hebrew, Arabic, Armenian or CJK text they follow
  (or else precede) if it has them, as browsers do; otherwise, and next
  to Latin, Greek or Cyrillic text, from the first font. Spaces always
  come from the first font.

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
- YAML front matter (see below).

### Front matter

A document may start with YAML front matter, as Jekyll, Hugo and Pandoc
read it: a `---` line, then YAML that starts with a `key:` line, then a
`---` or `...` line. By default it is left out, with a warning unless
`--no-front-matter` (or `front-matter = false` in `.sundowner`) says so.
With `--front-matter` (or `front-matter = true`) it is shown as a table
at the start of the document, for information: each level of nesting is
a column, a key spans the rows of its value, and the items of a list are
rows of their own (numbered, if they are mappings or lists themselves).
Values are shown as written, as plain text.

```yaml
title: Field notes
author:
  name: A. Person
  email: a@example.com
tags: [birds, spring]
```

| | | |
|---|---|---|
| **title** | Field notes | |
| **author** | **name** | A. Person |
| | **email** | a@example.com |
| **tags** | birds | |
| | spring | |

A `lang` key (a language tag such as `en` or `en-US`, as Pandoc, Quarto
and Jekyll read it) gives the document's language, whether or not the
front matter is shown; see *Justification and hyphenation* below.

Each column needs 6 ems of the page's width (with the default margins
and font size, seven columns fit on A4 and four on A5); front matter
nested deeper than fits is left out with a warning. So is front matter that sundowner cannot read:
it reads the YAML that front matter is written in (block and flow
mappings and lists, plain, quoted and block scalars, comments), but not
anchors, aliases or complex keys.

The PDF also gets a bookmark outline built from the headings, clickable links,
page numbers and a document title. Only `http:`, `https:` and `mailto:` links
and `#anchor` links become clickable; other links are shown as plain text.
It is a tagged PDF, which screen readers can read, and a PDF/A file, for
archiving; see *Tagged PDF* and *PDF/A* below.

## Design

- **No dependencies.** Everything is in `src/`: the Markdown parser, layout
  engine, Unicode line breaker and normalizer, TrueType parser and
  subsetter, OpenType glyph substitution and positioning, PDF writer, DEFLATE codec and PNG/JPEG readers. The bundled fonts are compiled into the binary.
- **Self-contained PDFs.**
  - Every font is embedded as a subset (Type 0 / CIDFontType2 with
    Identity-H encoding), so viewers never substitute fonts.
  - A ToUnicode map keeps copy-paste and search working.
  - A one-word document is about 4 KB.
  - Subset names are derived from their contents, so identical input gives
    byte-identical output.
- **Tagged PDF.** The PDF carries the document's logical structure
  (ISO 32000-1, 14.8), which screen readers, reflowing viewers and text
  extraction follow: headings (`H1` to `H6`), paragraphs, lists (`L`,
  `LI`, `Lbl`, `LBody`), tables (`TH` cells head their column, or their
  row for front matter keys, with row and column spans), block quotes,
  code blocks, figures with their alt text, and links, each tied to its
  annotation. Every piece of page content is marked as part of an element
  or as an artifact: backgrounds, rules, the bars of quotes and page
  numbers are artifacts, and so is an image without alt text, as
  decoration.

  Lines are drawn in visual order, as readers expect: they put
  right-to-left text in logical order themselves (pdftotext and MuPDF
  read Hebrew correctly this way, and turn it around if it is given in
  logical order, even as `ActualText`). The structure places the pieces
  of a line in its elements in logical order, so a link in the middle of
  a Hebrew sentence is read where it belongs. Where a glyph's Unicode
  mapping would give the wrong text, that glyph carries its text as
  `ActualText`: a hyphen added where a line breaks inside a word stands
  for a soft hyphen (U+00AD), so search finds the whole word, and a
  character that no font has, drawn as the outline of a box, keeps its
  character. Ligatures already map back to their characters (see
  *Ligatures* below).
- **PDF/A.** Every PDF is also PDF/A-2 (ISO 19005-2), the archival form
  of PDF: at level A (accessible), as it is tagged and all its text maps
  to Unicode, or at level B where a font sets a character with several
  glyphs, some of which stand for no text of their own. It holds XMP
  metadata (the title, language and producer, as in its document
  information) and an sRGB output intent with a 1 KB ICC profile that
  sundowner generates. Its file identifier is a hash of its contents (up to the
  cross-reference stream, which holds it), so
  identical input still gives byte-identical output. A CMYK JPEG cannot be
  in a PDF/A file with RGB colours; such a PDF is not declared PDF/A, with
  a warning. The example documents pass [veraPDF](https://verapdf.org)'s
  PDF/A-2a validation.

  They are not declared PDF/UA (ISO 14289), the standard for accessible
  PDF, as that asks for more than a Markdown file guarantees, such as a
  language and headings that do not skip levels. A document that has
  those (and alt text for its images) passes veraPDF's PDF/UA-1 checks,
  apart from the declaration.
- **Bidirectional text.** Hebrew and other right-to-left text is laid out
  with the Unicode Bidirectional Algorithm
  ([UAX #9](https://www.unicode.org/reports/tr9/), Unicode 17.0) in full,
  including explicit embeddings, overrides and isolates (U+202A–U+202E,
  U+2066–U+2069) and paired brackets, and passes both official
  conformance tests. Each paragraph, heading, list item and table cell
  takes its direction from its first strong character, like GitHub's
  `dir="auto"`: right-to-left paragraphs are set flush right, and
  right-to-left list items and quotes are indented from the right, with
  their bullets, numbers and bars on the right. Numbers and Latin words
  inside Hebrew stay left-to-right, and brackets are mirrored (`(` shows as
  `)` in right-to-left text). Lines are broken first and then reordered,
  as the algorithm prescribes. Code blocks are left-to-right, with
  right-to-left parts reordered. Table columns keep their order; cells
  without an explicit alignment follow their text's direction.
- **Arabic shaping.** Letters join as the Unicode Standard's cursive
  joining model prescribes (from the joining types of the Unicode
  Character Database), across vowel marks and changes of style, with the
  zero-width joiner and non-joiner honoured. The font's GSUB features are
  applied in HarfBuzz's stages for Arabic: `rtla` and `rtlm`, `ccmp` and
  `locl`, then the positional forms `isol`, `fina`, `medi` and `init`,
  each only to the letters in that form, then `rlig`, `calt` and `rclt`,
  and `liga`, `clig` and `mset`. Marks are ordered as HarfBuzz orders
  them (shadda before the vowels, hamza next to its letter). With Amiri,
  glyphs and positions match HarfBuzz, in all four styles, for 153 words
  of Arabic, Persian, Urdu and fully vocalized Quranic text: a test
  checks all 612 cases against data from `tools/gen_harfbuzz.py`.
- **Ligatures and contextual forms.** Glyph substitutions come from the
  font's OpenType GSUB table, with the features HarfBuzz applies by default
  to scripts without script-specific shaping (Latin, Greek, Cyrillic,
  Armenian, Hebrew, Chinese, Japanese and Korean): `ccmp`, `locl`, `rlig`,
  `liga`, `clig`, `calt`, `rclt` and `rvrn` (and `rtla` and `rtlm` for
  Hebrew), from the language system of the text's script.
  All GSUB lookup types are supported, including chained contextual and
  reverse chained substitutions, along with the lookup flags that skip
  marks or ligatures. For the bundled fonts the result matches HarfBuzz
  glyph for glyph. Ligatures map back to their characters in the ToUnicode
  map, so copying `office` from the PDF gives `office`. A zero-width
  non-joiner (U+200C) prevents a ligature. Code gets the same treatment, so
  programming fonts with ligatures show them; Cousine has none.
- **Combining marks.** A letter and the combining marks after it are set
  together, in the first font that covers them all. They are normalized
  for that font, as HarfBuzz does: marks go in canonical order, and a
  letter and its marks become the precomposed character where the font has
  one (`e` + U+0301 is drawn as the designed `é`). Where there is no
  precomposed glyph, the marks take no space and are attached to the letter,
  or stacked on each other, by the font's anchors (the GPOS `mark` and `mkmk`
  features: mark-to-base, mark-to-ligature and mark-to-mark). Fonts without
  anchors get marks centred over or under the letter from the glyph
  outlines. Kerning looks past marks. Hebrew points are ordered as HarfBuzz
  orders them (shin and sin dots, dagesh, vowels, meteg), and for fonts
  without mark positioning, such as Cousine, a letter and its points
  become the Hebrew presentation form where the font has one. For the
  bundled fonts, glyphs and positions match HarfBuzz, in right-to-left
  text too.
- **Kerning.** Pairs of adjacent glyphs are kerned using the font's
  OpenType `kern` feature (GPOS pair adjustment). Fonts without GPOS
  kerning fall back to the legacy `kern` table. Kerning is included when
  measuring text, so line breaks account for it. It applies to text in one
  font, not across spaces or font changes. As the OpenType spec prescribes,
  adjustments from all of the font's kerning lookups add up.
- **Glyph positioning.** All right-to-left text (Hebrew as well as
  Arabic), and text in fonts that use more than pair kerning and mark
  attachment for its script, is positioned with all of the GPOS table as
  HarfBuzz applies it: single and pair adjustment, cursive attachment
  (which connects Arabic letters), mark attachment and contextual and
  chained contextual positioning, from the `kern`, `mark`, `mkmk`, `curs`,
  `dist`, `abvm` and `blwm` features. Device tables, which adjust
  positions for a given pixel size, are ignored; HarfBuzz ignores them too
  unless it is given a pixel size, which output like PDF has none of.
- **Line breaking** follows the Unicode Line Breaking Algorithm
  ([UAX #14](https://www.unicode.org/reports/tr14/), Unicode 17.0) in full,
  and passes its official conformance test. Lines break at spaces, after
  hyphens and dashes, and between CJK characters. They do not break before
  closing punctuation (`word !`, `« quote »`), inside numbers such as
  `$12.50`, or at no-break spaces. Break opportunities are computed over
  the whole paragraph, across style changes. A soft hyphen (U+00AD) marks
  a possible break, and shows a hyphen only if the line breaks there.
- **Justification and hyphenation.** Paragraphs are justified: every line
  but the last (and lines before a hard line break) is set flush with both
  margins by widening or narrowing the spaces between its words. Where
  their lines break is chosen for the whole paragraph at once, by Knuth
  and Plass's total-fit algorithm as TeX does it: of all the ways to break
  a paragraph, the one whose spaces stretch or shrink the least (each
  line's badness grows with the cube of it), with extra cost for
  hyphenated lines, two of them in a row, and very loose lines next to
  tight ones. Spaces may shrink to 75 % of their width, and are widened as
  far as needed, up to four times their width; a line that would need
  more (as when a long URL follows it) is set flush with the start
  instead. Headings, table cells and code are not justified, and
  `--no-justify` sets paragraphs ragged, with the breaks still chosen for
  an even edge. Right-to-left paragraphs are justified too, before they
  are reordered.

  Arabic is justified by kashida first: words are lengthened by tatweels
  (U+0640) between two joined letters, as far as they fit, and the spaces
  take what is left. A word gets them in one place, chosen by the classic
  priorities (after seen or sad; before a final teh marbuta or heh; before
  a final reh or dal; before a final alef, tah, lam or kaf; after beh and
  the letters like it; before a final waw, ain, qaf or feh; before any
  other final letter), never inside lam-alef or a ligature, and by at most
  one em; each word on a line by one tatweel before any by two. The line
  breaker counts kashidas as stretch. Tatweels are text: text read from
  the PDF has them, as it does from Word and LibreOffice.

  Text is hyphenated if its language is known (from `--lang`, the front
  matter's `lang`, or `lang` in `.sundowner`, in that order), by Liang's
  algorithm with the TeX hyphenation patterns of
  [hyph-utf8](https://github.com/hyphenation/tex-hyphen): so far for
  English, with the American patterns for every `en` tag. Hyphenation is
  never guessed, as patterns for one language break the words of another
  in the wrong places. Only whole words are hyphenated, with at least two
  letters before the hyphen and three after it: not code, words with
  digits or capitals after the first letter (acronyms, camel case), words
  joined by a hyphen or slash, or URLs. Nor is a word hyphenated where it
  would split a ligature (not "of-fice", which would lose its "ffi").
  Without hyphenation, at A4 with the default margins and font size
  (about 95 characters a line), the spaces of a justified line of prose
  are typically 1.15 times their width, and 1 line in 10 has them wider
  than 1.5 times; with English hyphenation, 1 line in 50, with a hyphen at
  the end of 1 line in 10. On narrow pages such as A5, hyphenation matters
  more: without it, a third of the lines have spaces wider than 1.5 times.
- **Page breaking** chooses the breaks for the whole document at once, as
  Knuth and Plass's algorithm chooses the line breaks of a paragraph. Each
  place a page may break has a penalty: after the first or before the last
  line of a paragraph (widows and orphans), inside a code block, quote,
  list item or table (a table row is kept together unless it is taller
  than half a page), and above all right after a heading, while breaks
  before a section heading are encouraged. Each page also costs the cube
  of its empty fraction, except the last page, whose empty space is free.
  So when there is room at the end of the document, a block that would be
  split moves whole to the next page, and the space it leaves falls at the
  bottom of a page instead of the end of the document. A page costs more
  than any bad break except one right after a heading or a table's header
  row, so avoiding bad breaks does not add pages. The document is laid out
  twice: once to find the breaks, and again to set the pages, reusing the
  lines of the first pass.
- **Compact output.** Content streams and fonts are compressed with a
  small built-in DEFLATE encoder: LZ77 with hash chains and lazy matching,
  and each block coded with Huffman codes built for it (length-limited by
  package-merge), the fixed code, or stored, whichever is smallest. It
  compresses as well as zlib at its highest level, to within a fraction
  of a percent. The other objects (pages, fonts, the structure tree) are
  packed into compressed object streams, with a compressed
  cross-reference stream (PDF 1.5), which keeps the structure tree from
  adding more than about 6 % to a document.
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
- **Fast.** About 550 KB of Markdown (600 pages) converts in about 0.3 s
  using about 40 MB of memory. Justification adds about 5 % to that, and
  hyphenation 15 to 20 %.

## Limitations

- **Scripts.** Latin, Greek, Cyrillic, Armenian, Hebrew, Arabic and CJK
  render correctly. There is none of the shaping that Syriac, N'Ko, the
  Indic scripts, Thai or Mongolian need. Arabic letters do not take
  language-specific forms (such as Urdu's or Sindhi's). Fonts without OpenType Arabic features are not shaped from
  the presentation forms.
- **Traditional Chinese characters.** Markdown does not say which
  language text is in, so the silk build sets Chinese characters in one
  style: the traditional (Taiwan) forms of Noto Serif TC. Japanese text is
  complete too: the kanji that were simplified in Japan (such as 気, 楽
  and 読), which Noto Serif TC lacks, come from Noto Serif JP, which is
  bundled cut down to just those. Kanji that Japan and Taiwan share keep
  their Taiwan shapes, and there is no vertical text.

  Simplified Chinese characters that traditional Chinese does not use
  (such as 这, 们 and 语) are not bundled, and are shown as boxes with a
  warning. For simplified Chinese, add
  [Noto Serif SC](https://fonts.google.com/noto/specimen/Noto+Serif+SC)
  (or another simplified Chinese font) as a fallback in `.sundowner`:

  ```ini
  [fallback]
  regular = fonts/NotoSerifSC-Regular.ttf
  bold = fonts/NotoSerifSC-Bold.ttf
  ```

  Fallbacks come before the bundled fonts, so all Chinese characters
  (and kana and full-width punctuation) are then set in Noto Serif SC,
  in mainland China forms, while Latin text stays in Alegreya. Noto Serif
  JP added the same way sets Japanese entirely in Japanese forms, and
  Noto Serif HK in Hong Kong ones. As `.sundowner` is looked up from each
  document's directory, a folder of simplified Chinese documents can have
  its own; `-c` picks one for a single document, and `--no-config` goes
  back to the bundled fonts.
- **Cantillation marks** in Hebrew come from Noto Serif Hebrew, so in
  cantillated (biblical) text the letters that carry them are set in Noto
  Serif Hebrew and the others in Frank Ruhl Libre.
- **CJK in code** is set in Noto Serif TC, which is proportional, so CJK
  characters in code do not line up in columns. Add a monospaced CJK font
  as `[mono]` or `[fallback]` if that matters.
- **Marks on ligatures** attach to the ligature's last component. A mark
  that belonged to an earlier component, such as an accent on the f of an
  "fi" ligature, is placed on the last one instead.
- Remote images, interlaced PNGs and formats other than PNG and JPEG are not
  embedded. They appear as an italic `[image: …]` placeholder with a warning.

- **Hyphenation** is only for English so far, and for the whole
  document: a passage in another language is hyphenated as if it were
  English, where its letters are ones English uses. CJK text, which has
  no spaces to widen, is not justified.

[`IDEAS.md`](IDEAS.md) collects features that may come later, such as
hyphenation for more languages.

## Building

```sh
# Fully static Linux binary (about 3.2 MB, most of it the bundled fonts)
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
# -> target/x86_64-unknown-linux-musl/release/sundowner

# The silk build (about 20 MB)
cargo build --release --target x86_64-unknown-linux-musl --features silk

# Regular build for the host platform
cargo build --release

cargo test                                   # quick
cargo test --release -- --include-ignored    # plus the stress tests (as in CI)
cargo test --release --features silk -- --include-ignored
```

The silk fonts are compressed by `build.rs`, with sundowner's own DEFLATE
encoder, when the feature is enabled; this adds a few seconds to the
build. A document that uses them pays about 0.08 s to unpack each of the
large CJK fonts it needs.

The Unicode tables in `src/linebreak_table.rs`, `src/normalize_table.rs`
and `src/bidi_table.rs` are generated from the Unicode Character Database by `tools/gen_unicode.py`
(Python 3, standard library only). It also writes the conformance test
data in `tests/data/`. To move to another Unicode version, change `VERSION`
in the script and run it.

`tools/gen_harfbuzz.py` (Python 3 with `uharfbuzz`) shapes the words of
`tests/data/arabic-words.txt` with HarfBuzz and writes the glyphs and
positions `tests/arabic.rs` expects. Run it after changing the word list,
Amiri or the HarfBuzz version.

Requires Rust 1.87 or newer (the code uses `u*::is_multiple_of`, stabilized
in 1.87). CI checks formatting, clippy, the full test suite, the static build
and the minimum Rust version.

## License

The code is licensed under the MIT License (see `LICENSE`). The bundled
fonts in `fonts/` are licensed under the SIL Open Font License 1.1. See
[`fonts/README.md`](fonts/README.md) for their sources, changes and license
compliance, or run `sundowner --licenses`. The hyphenation patterns in
`hyph/` come with their own permissive licenses; see
[`hyph/README.md`](hyph/README.md). The line breaking,
normalization and bidirectional tables and their test data are derived from the Unicode
Character Database, under the Unicode License v3 (see `LICENSE-UNICODE`).

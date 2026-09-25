# sundowner

Markdown to PDF converter. A single static binary with no runtime or crate
dependencies. It is written in safe Rust (`#![forbid(unsafe_code)]`) and uses
only the standard library.

```sh
sundowner notes.md                 # writes notes.pdf
sundowner *.md                     # one PDF per input
sundowner -o out.pdf < in.md       # stdin -> file
cat in.md | sundowner > out.pdf    # stdin -> stdout
```

## Options

```
-o, --output <FILE>     Output file ('-' for standard output); single input only
-p, --paper <SIZE>      a4 (default), a5, a3, letter, legal, or WIDTHxHEIGHT in mm
-f, --font <FAMILY>     sans (default) or serif
-s, --font-size <PT>    Body font size in points (default 11)
-m, --margin <MM>       Page margin in millimetres (default 20)
-t, --title <TEXT>      Document title (default: first level-1 heading)
    --no-page-numbers   Do not number pages
    --no-images         Do not load local image files
-q, --quiet             Do not print warnings
```

Exit status: `0` on success, `1` if any input failed, `2` on usage errors.

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

- **No dependencies.** Text uses the 14 standard PDF fonts (Helvetica, Times,
  Courier), so no fonts are embedded and small documents stay a few KB.
  The Markdown parser, layout engine, PDF writer, DEFLATE codec and PNG/JPEG
  readers are all in `src/`.
- **Compact output.** Content streams are compressed with a small built-in
  DEFLATE encoder (LZ77 with short hash chains and the fixed Huffman code).
  This is simple and fast but compresses less than zlib or gzip at their
  default settings.
- **Crash-proof.**
  - No `unsafe` code.
  - Nesting depth is capped.
  - Every parser scan is bounded, so hostile input cannot hang the parser
    (the CommonMark delimiter algorithm keeps emphasis parsing linear).
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
- **Fast.** About 550 KB of Markdown (800+ pages) converts in about 0.35 s.

## Limitations

- The standard PDF fonts only cover Windows-1252 (Western European)
  characters. Common typographic symbols are mapped or transliterated
  (arrows, ≤, ≥, ligatures, Latin Extended-A letters). Other scripts such as
  CJK and emoji are replaced with `?`.
- Line wrapping works on those Windows-1252 bytes and breaks at spaces. It is
  not Unicode-aware (no grapheme or script-specific line breaking).
- Remote images, interlaced PNGs and formats other than PNG and JPEG are not
  embedded. They appear as an italic `[image: …]` placeholder with a warning.

## Building

```sh
# Fully static Linux binary (about 700 KB)
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
# -> target/x86_64-unknown-linux-musl/release/sundowner

# Regular build for the host platform
cargo build --release

cargo test                                   # quick
cargo test --release -- --include-ignored    # plus the stress tests (as in CI)
```

Requires Rust 1.87 or newer (the code uses `u*::is_multiple_of`, stabilized
in 1.87). CI checks formatting, clippy, the full test suite, the static build
and the minimum Rust version.

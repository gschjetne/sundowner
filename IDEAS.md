# Ideas

Features that would fit sundowner, with notes on how they would fit the
code as it is. Nothing here is promised; it is a place to think ahead.

## Justification, hyphenation and kashida

Text is set ragged (flush left, or flush right for right-to-left
paragraphs), and lines are filled greedily. These three belong together:
justified text needs somewhere to put the leftover space on each line, and
the better the line breaks, the less of it there is. Western scripts take
it up in the spaces between words and cut it down with hyphenation; Arabic
takes it up inside words, by lengthening the joins between letters
(kashida). All three need to know where a line is best broken, so they are
best built on one line breaker.

### Line breaking

- Replace the greedy `wrap` with total-fit line breaking (Knuth and Plass):
  choose the breaks for the whole paragraph that minimize the sum of the
  lines' badness, where badness grows with how far each line's spaces are
  stretched or shrunk from their natural width. The feasible breaks are
  the UAX #14 opportunities that `linebreak::opportunities` already
  computes, so the Unicode rules still decide where a line may end.
- Penalties: a break at a hyphen (soft or automatic) costs more than one
  at a space; two hyphenated lines in a row cost extra; so does a very
  loose line next to a very tight one.
- Keep greedy breaking for ragged text, or use total-fit there too with a
  fixed space width, which evens out the ragged edge.
- Cost: the dynamic program is linear in practice with the usual active
  node pruning. Documents of hundreds of pages must stay fast (the README
  promises 600 pages in about 0.25 s).

### Western justification

- A `justify = true` setting (and `--justify`), off by default so output
  stays what it is today.
- The spaces between words are already frags without glyphs (see `wrap`),
  so justifying a line means widening those frags before it is drawn.
  Stretch limits come from the space width: for example shrink to 80 %
  and stretch to 150 %, and more only when there is no better break.
- Not justified: the last line of a paragraph, lines ending at a hard
  break, headings, table cells, code and lines with a single word
  (set those flush with the start).
- Bidirectional text: widen the gaps before reordering, so the reordered
  line comes out justified too.
- CJK has no spaces: spread the space between characters instead, and
  compress full-width punctuation first (as JLREQ and CLREQ describe:
  `、。「」` are half empty).
- Letter spacing as a last resort is not wanted for Latin; better an
  underfull line.

### Hyphenation

- Liang's algorithm with the TeX hyphenation patterns (the `hyph-utf8`
  collection). Patterns compile to a compact trie at build time, as the
  Unicode tables are generated today, and ship only for the languages of
  the build (so europa gets the Latin, Greek and Cyrillic ones). Check each
  pattern file's license: most are MIT, LPPL or public domain, and each
  one embedded needs its notice in `--licenses`.
- Markdown does not say which language text is in, so this needs a `lang`
  setting (config and command line), later perhaps a per-document one from
  front matter. The same setting would be useful elsewhere: see
  *Language-specific forms* below.
- Hyphenation points become extra break opportunities that show a hyphen
  when taken; soft hyphens already work this way (`Word::hyphen`), so the
  drawing side is done.
- Rules: minimum word length and characters before and after the hyphen
  (TeX's `\lefthyphenmin` and `\righthyphenmin`, per language), never in
  code, URLs or words with digits, and a limit on consecutive hyphenated
  lines (a penalty in the line breaker).
- The hyphen character depends on the script: `-` for Latin and Cyrillic,
  the Armenian hyphen `֊` for Armenian. Hebrew is traditionally not
  hyphenated.

### Kashida

- Arabic justification lengthens the connection between two joined
  letters, by inserting tatweel (U+0640) or by using wider alternates of
  the letters where the font has them, instead of (or before) widening the
  spaces. Words are never broken across lines.
- Where a kashida may go: only between two letters that are joined (the
  joining state machine in `arabic::forms` knows which), and by the
  classic priorities: after seen and sad, before a final teh marbuta or
  heh, before a final reh or dal, after beh-like letters, and so on; at
  most one per word, and never inside lam-alef or after the last letter.
  HarfBuzz records the same information as `SAFE_TO_INSERT_TATWEEL`, which
  our joining code can mirror.
- Tatweel is join-causing, so inserting it does not change the letters'
  forms; the run is shaped again with it, and with Amiri the tatweel
  connects by cursive attachment. Its width comes in steps, so the rest of
  the slack still goes into the spaces.
- Fonts with a `jalt` feature or stretchable glyph variants could use
  those instead of tatweel; worth trying with Amiri.
- The line breaker needs to know how much a line can stretch by kashida,
  so it counts the kashida points of each line as stretchability.

## Page breaking

- Repeat a table's header row at the top of each page it continues on.
  The page breaker would count the header's height at every break between
  rows.
- An explicit page break, for example a `<!-- pagebreak -->` comment: a
  breakpoint with a penalty that forces a break (TeX's `\penalty-10000`).
- Keep the lines of a table row together only up to a limit, as a long
  cell that fills most of a page is better split than moved.

## Language-specific forms

- With a `lang` setting (see Hyphenation), apply the font's `locl`
  lookups for that language: Urdu, Sindhi, Kashmiri and Malay forms in
  Amiri; Serbian and Bashkir Cyrillic and Turkish, Catalan, Dutch,
  Romanian and other Latin forms in Alegreya (such as Serbian italic
  letters, the Catalan `l·l` and the Dutch `ij`).
- Regional forms of Chinese characters: the silk build uses the
  traditional (Taiwan) forms of Noto Serif TC for everything, with only the
  kanji it lacks from Noto Serif JP. A Japanese, simplified Chinese or
  Korean `lang` could select Noto Serif JP, SC or KR, perhaps as an
  optional larger build, since each is 10 to 15 MB; today such fonts are
  added as fallbacks.

## More scripts

- Syriac and N'Ko: they join like Arabic, and `arabic.rs` already has the
  machinery (Syriac needs the extra Alaph and Dalath-Rish states HarfBuzz
  has).
- The Indic scripts, Southeast Asian scripts and others with syllable
  clusters: the Universal Shaping Engine, as HarfBuzz implements it.
- Thai, Lao, Khmer and Myanmar break lines between words that are not
  separated by spaces, which needs a dictionary; today they only break at
  spaces.
- Vertical CJK text, and Mongolian, which is only written vertically.
- Arabic from presentation forms, for fonts that have them but no
  OpenType Arabic features (HarfBuzz's fallback shaping).

## Shaping details

- Marks on ligatures attach to the ligature's last component; tracking
  which component a mark belonged to (as HarfBuzz does with ligature IDs)
  would place it on the right one.
- Use the full GPOS engine (`position.rs`) for all text, not only
  right-to-left text and fonts that need it, once it is shown to give
  identical output and speed for the bundled fonts. Then `kern.rs` and
  the mark attachment in `gpos.rs` could go.
- A monospaced CJK font for code, so CJK in code lines up in columns.

## Output

- Tagged PDF, with the reading order and `ActualText` of right-to-left
  and ligated text, so that copying, searching and screen readers get the
  logical text rather than the visual order.
- PDF/A for archiving: the fonts are embedded already; it also needs
  output intents and XMP metadata.
- Dynamic Huffman codes in the DEFLATE encoder: smaller PDFs, and a
  smaller silk binary, since `build.rs` compresses the silk fonts with it.
  A faster inflater would also cut the time to unpack a CJK font on first
  use (about 0.15 s).

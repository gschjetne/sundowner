# Ideas

Features that would fit sundowner, with notes on how they would fit the
code as it is. Nothing here is promised; it is a place to think ahead.

## Justification and hyphenation

Paragraphs are justified, with breaks chosen by total fit (`total_fit` in
`layout.rs`), and hyphenated by Liang's algorithm (`hyphenate.rs`) when
the language is known; so far only English. What could follow:

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
- The line breaker takes it as stretch: `FitWord` would get the stretch
  of its kashida points, which `fit_lines` adds to that of the spaces,
  and `justify` would give each line's slack to the kashidas first.

### More languages

Patterns for the main European languages, from hyph-utf8 (sizes of the
`.pat.txt` files; "gz" is gzip -9):

| Language | Tag | Patterns | Size (gz) | License | Also needs |
|---|---|---|---|---|---|
| German | de-1996 | 36,709 | 265 KB (117) | MIT | traditional spelling (de-1901) splits "ck" as "k-k" and restores triple consonants ("Schiff-fahrt") |
| French | fr | 1,216 | 9 KB (3) | MIT | hyphenate after an elision ("l'infor-mation"); no-break thin spaces before `;:!?` and inside « » |
| Spanish | es | 4,694 | 39 KB (12) | MIT | |
| Italian | it | 384 | 1 KB | LPPL | elisions, as French |
| Portuguese | pt | 427 | 2 KB | BSD | repeat the hyphen of a compound at the start of the next line ("guarda-/-chuva") |
| Catalan | ca | 869 | 5 KB | LPPL | "l·l" breaks as "l-/l"; elisions |
| Dutch | nl | 12,724 | 81 KB (39) | MIT | |
| Swedish | sv | 4,693 | 28 KB (14) | LPPL | |
| Danish | da | 1,144 | 6 KB | LPPL | |
| Norwegian | nb, nn | 27,448 | 188 KB (91) | permissive | the same file for both |
| Finnish | fi | 286 | 1 KB | "freely distributed" | |
| Icelandic | is | 4,188 | 25 KB (13) | LPPL | |
| Estonian | et | 3,691 | 22 KB (11) | MIT | |
| Latvian | lv | 11,583 | 82 KB (32) | LGPL or GPL | |
| Lithuanian | lt | 1,546 | 8 KB | MIT | |
| Polish | pl | 4,053 | 29 KB (10) | MIT | repeated hyphen, as Portuguese; one-letter words (w, z, i) should not end a line |
| Czech | cs | 3,636 | 21 KB (10) | GPL 2+ | as Polish (v, k, s, z, o, u, a, i) |
| Slovak | sk | 2,467 | 18 KB (7) | MIT | as Czech |
| Hungarian | hu | 62,851 | 515 KB (193) | MPL 1.1 or GPL | long double consonants split as their letters: "asszony" as "asz-szony" |
| Romanian | ro | 647 | 3 KB | none stated | |
| Croatian | hr | 1,475 | 7 KB | LPPL | repeated hyphen |
| Slovenian | sl | 1,068 | 5 KB | LPPL | repeated hyphen |
| Serbian | sh-latn, sr-cyrl | 2,669 / 2,425 | 20 / 27 KB | LPPL / GPL | Cyrillic could come from the Latin patterns by transliteration (lj, nj, dž are љ, њ, џ) |
| Russian | ru | 7,021 | 61 KB (21) | LPPL | |
| Ukrainian | uk | 4,564 | 42 KB (13) | MIT | |
| Belarusian | be | 3,298 | 27 KB (6) | MIT | |
| Bulgarian | bg | 6,886 | 60 KB (17) | BSD-like | |
| Greek | el-monoton, el-polyton | 573 / 1,208 | 3 / 11 KB | LPPL | choose by tag (`el`, `el-polyton`, `grc`) |
| Welsh | cy | 6,728 | 42 KB (20) | LPPL | |
| Irish | ga | 6,033 | 42 KB (19) | GPL 2+ | |
| Turkish | tr | 597 | 2 KB | LPPL | |

- Size: most are small; German, Norwegian and Hungarian are most of
  the total (about 1.6 MB for all of these, 0.7 MB gzipped). They could
  be stored compressed, as the silk fonts are, and inflated when a
  document needs them; or only the small ones bundled in europa.
  Building the trie at first use takes 1.5 ms for English, 15 ms for
  German and 20 ms for Hungarian; looking up a word takes a few
  microseconds whatever the language, so a precompiled trie (generated
  like the Unicode tables) is only worth it for the big ones.
- Licenses: GPL patterns (Czech, Macedonian, Serbian Cyrillic, Irish; and
  Latvian and Hungarian, which offer it as one choice) cannot go into an
  MIT binary as they are; Hungarian could use its MPL, and the others
  would need their authors' permission or other patterns.
- Hyphenation that changes the letters (German 1901, Hungarian, Catalan)
  needs a word to show different text before and after the break: a
  `Word` whose hyphen frag carries the replacement, and whose next piece
  is shaped with the restored letter.
- The rules around words: elisions (strip "l'", "d'", "dell'" before
  hyphenating), the repeated hyphen, and no-break spaces after one-letter
  words, all by language.
- Per-passage language: HTML `lang` attributes on inline tags (now
  dropped) could switch patterns for a quotation.

### Other

- Keep ligatures at hyphenation points, as TeX does: "of-fice" is not
  hyphenated today, so that "office" keeps its "ffi". The word would be
  shaped whole where it is not broken, and in pieces where it is.
- CJK has no spaces: spread the space between characters instead, and
  compress full-width punctuation first (as JLREQ and CLREQ describe:
  `、。「」` are half empty). Today CJK lines are set flush with the start.
- Hyphenate table cells, which are narrow; their column widths are chosen
  from the longest word, which hyphenation would change.

## Page breaking

- Repeat a table's header row at the top of each page it continues on.
  The page breaker would count the header's height at every break between
  rows.
- An explicit page break, for example a `<!-- pagebreak -->` comment: a
  breakpoint with a penalty that forces a break (TeX's `\penalty-10000`).

## Language-specific forms

- With the `lang` setting (see Justification and hyphenation), apply the
  font's `locl` lookups for that language: Urdu, Sindhi, Kashmiri and
  Malay forms in Amiri; Serbian and Bashkir Cyrillic and Turkish, Catalan, Dutch,
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

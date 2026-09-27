# Hyphenation patterns

Patterns for Liang's hyphenation algorithm, compiled into the sundowner
binary and used when a document's language is known (`--lang`, `lang =` in
`.sundowner`, or `lang:` in front matter). They come from the
[hyph-utf8](https://github.com/hyphenation/tex-hyphen) collection of TeX
hyphenation patterns, in its plain-text form (`patterns/txt/`), and are
byte-for-byte upstream.

| Files | Language | Patterns | License |
|-------|----------|----------|---------|
| `hyph-en-us.pat.txt`, `hyph-en-us.hyp.txt` | English (American spelling), used for all `en` tags | 4938 patterns, 14 exceptions, 31 KB | Permissive (Kuiken), [`LICENSE-en-us.txt`](LICENSE-en-us.txt) |

`sundowner --licenses` shows each license.

# Sundowner Sample

A **fast**, *dependency-free* Markdown → PDF converter. It handles ***nested
emphasis***, `inline code`, ~~strikethrough~~, [links](https://example.com)
and <https://autolinks.example.org>, plus “smart quotes” — and €uro signs.
Line one with a hard break  
line two.

## Contents

- [Lists](#lists)
- [Code](#code)
- [Tables](#tables)

## Lists

1. First item
2. Second item with a longer text that needs to wrap onto the following line because it is long enough to do so
   - nested bullet
   - another
     1. deep ordered
3. Third

- [x] Done task
- [ ] Open task

## Code

```rust
fn main() {
    println!("Hello, world!"); // a comment that is quite long and will need to be wrapped because it exceeds the width of the page
}
```

    indented code block

> A blockquote with **bold** text.
>
> > Nested quote.

## Tables

| Feature        | Supported | Notes                        |
|:---------------|:---------:|-----------------------------:|
| Headings       | yes       | ATX and setext               |
| Tables         | yes       | alignment, wrapping of long cell contents across lines |
| Images         | yes       | PNG & JPEG                   |

---

![Sunset](sunset.png)

Setext heading
--------------

Some text with a footnote-like [reference link][ref] and a missing image
![nothing](missing.png).

[ref]: https://commonmark.org "CommonMark"

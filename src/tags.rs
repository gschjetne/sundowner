//! Tagged PDF: the logical structure of a document (its headings,
//! paragraphs, lists, tables and figures, in reading order), and the marked
//! content that ties what is drawn on the pages to it. Screen readers,
//! reflowing viewers and text extraction read the document through it.
//!
//! Layout builds the structure tree as it sets the blocks, and marks each
//! piece of page content as belonging to an element (a marked-content
//! sequence with an ID, the MCID, unique on its page), or as an artifact:
//! decoration such as backgrounds, rules and page numbers, which is not
//! part of the text.

use std::io::Write;

/// A structure element.
pub struct Elem {
    /// Its structure type, such as `P`, `H1`, `LI` or `TD`.
    pub tag: &'static str,
    /// The element it is in; the root's is itself.
    pub parent: usize,
    pub kids: Vec<Kid>,
    /// Alternate description, of a figure.
    pub alt: Option<String>,
    /// Attributes, as entries of an attribute dictionary (such as
    /// `/O /Table /Scope /Column`), or empty.
    pub attrs: String,
}

/// What a structure element contains, in reading order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kid {
    Elem(usize),
    /// A marked-content sequence on a page.
    Content {
        page: usize,
        mcid: usize,
    },
    /// A link annotation: the page's `index`-th link area.
    Link {
        page: usize,
        index: usize,
    },
}

/// What the content being added to the last page is marked as.
#[derive(Clone, Copy, PartialEq)]
enum Open {
    Nothing,
    Elem(usize),
    Artifact,
}

pub struct Tagger {
    /// The elements; the root, `Document`, is the first.
    pub elems: Vec<Elem>,
    /// For each element, where each of its kids goes in reading order (see
    /// [`Tagger::place`]).
    keys: Vec<Vec<(u64, u32)>>,
    /// Where what is added now goes.
    key: (u64, u32),
    /// The elements that content goes to, innermost last.
    stack: Vec<usize>,
    /// For each page, the element of each of its marked-content sequences,
    /// by MCID.
    pub marked: Vec<Vec<usize>>,
    open: Open,
    /// The kid of the open marked content, in its element.
    open_kid: usize,
}

impl Default for Tagger {
    fn default() -> Self {
        Tagger {
            elems: vec![Elem {
                tag: "Document",
                parent: 0,
                kids: Vec::new(),
                alt: None,
                attrs: String::new(),
            }],
            keys: vec![Vec::new()],
            key: (0, 0),
            stack: vec![0],
            marked: Vec::new(),
            open: Open::Nothing,
            open_kid: 0,
        }
    }
}

impl Tagger {
    /// The element that content goes to now.
    pub fn current(&self) -> usize {
        *self.stack.last().unwrap_or(&0)
    }

    /// Content is added to elements in the order it is drawn, which within
    /// a line of right-to-left text is not the order it is read in. Kids
    /// added after this call go after those added before, and before those
    /// added after the next; among themselves, they are read in the order
    /// of their `index`, the least given to a marked-content sequence.
    pub fn place(&mut self, index: u32) {
        self.key.1 = index;
    }

    /// Start placing kids after all those so far (see [`Tagger::place`]).
    pub fn next_place(&mut self) {
        self.key = (self.key.0 + 1, 0);
    }

    fn push_kid(&mut self, elem: usize, kid: Kid) {
        self.elems[elem].kids.push(kid);
        self.keys[elem].push(self.key);
    }

    /// Add a link annotation to `elem`.
    pub fn add_link(&mut self, elem: usize, page: usize, index: usize) {
        self.push_kid(elem, Kid::Link { page, index });
    }

    /// Add an element to the end of `parent`.
    pub fn add(&mut self, parent: usize, tag: &'static str) -> usize {
        let id = self.elems.len();
        self.elems.push(Elem {
            tag,
            parent,
            kids: Vec::new(),
            alt: None,
            attrs: String::new(),
        });
        self.keys.push(Vec::new());
        self.push_kid(parent, Kid::Elem(id));
        id
    }

    /// Add an element to the current one, and send content to it until
    /// [`Tagger::end`].
    pub fn begin(&mut self, tag: &'static str) -> usize {
        let id = self.add(self.current(), tag);
        self.stack.push(id);
        id
    }

    /// Send content to an existing element until [`Tagger::end`].
    pub fn enter(&mut self, id: usize) {
        self.stack.push(id);
    }

    pub fn end(&mut self) {
        if self.stack.len() > 1 {
            self.stack.pop();
        }
    }

    /// Mark what is drawn next on page `page` (whose content is `ops`) as
    /// content of `elem`, continuing the marked-content sequence already
    /// open for it.
    pub fn mark(&mut self, ops: &mut Vec<u8>, page: usize, elem: usize) {
        if self.open == Open::Elem(elem) {
            let key = &mut self.keys[elem][self.open_kid];
            *key = (*key).min(self.key);
            return;
        }
        self.close(ops);
        if self.marked.len() <= page {
            self.marked.resize(page + 1, Vec::new());
        }
        let mcid = self.marked[page].len();
        self.marked[page].push(elem);
        self.open_kid = self.elems[elem].kids.len();
        self.push_kid(elem, Kid::Content { page, mcid });
        let _ = writeln!(ops, "/{} <</MCID {mcid}>> BDC", self.elems[elem].tag);
        self.open = Open::Elem(elem);
    }

    /// Mark what is drawn next as an artifact.
    pub fn artifact(&mut self, ops: &mut Vec<u8>) {
        if self.open != Open::Artifact {
            self.close(ops);
            ops.extend_from_slice(b"/Artifact BMC\n");
            self.open = Open::Artifact;
        }
    }

    /// The elements, with their kids in reading order.
    pub fn finish(mut self) -> Vec<Elem> {
        for (e, keys) in self.elems.iter_mut().zip(self.keys) {
            let mut kids: Vec<(usize, Kid)> = e.kids.drain(..).enumerate().collect();
            kids.sort_by_key(|&(k, _)| keys[k]);
            e.kids = kids.into_iter().map(|(_, kid)| kid).collect();
        }
        self.elems
    }

    /// End the marked content that is open, before the page ends or other
    /// content is written to it out of order.
    pub fn close(&mut self, ops: &mut Vec<u8>) {
        if self.open != Open::Nothing {
            ops.extend_from_slice(b"EMC\n");
            self.open = Open::Nothing;
        }
    }
}

/// A PDF text string: a literal string for printable ASCII, else UTF-16BE
/// in hex, which takes any Unicode text.
pub fn text_string(s: &str) -> String {
    if s.bytes().all(|b| (0x20..0x7F).contains(&b)) {
        let mut out = String::from("(");
        for c in s.chars() {
            if matches!(c, '(' | ')' | '\\') {
                out.push('\\');
            }
            out.push(c);
        }
        out.push(')');
        return out;
    }
    let mut out = String::from("<FEFF");
    for u in s.encode_utf16() {
        out.push_str(&format!("{u:04X}"));
    }
    out.push('>');
    out
}

/// Start a span whose text is `text` rather than what its glyphs map to
/// (the `ActualText` of ISO 32000-1, 14.9.4); end it with `EMC`.
pub fn begin_actual_text(ops: &mut Vec<u8>, text: &str) {
    let _ = writeln!(ops, "/Span <</ActualText {}>> BDC", text_string(text));
}

//! Hyphenation by Liang's algorithm, with the TeX hyphenation patterns of
//! the hyph-utf8 collection (see `hyph/README.md`).
//!
//! A pattern such as `1tio` or `.ach4` says, with its digits, how strongly
//! a word may or may not be broken at each place between its letters, for
//! the words it occurs in (`.` marks the start or end of a word). A word
//! may break where the highest digit of all the patterns that match there
//! is odd. The patterns are compiled into a trie the first time a document
//! in their language needs them, which takes about a millisecond; a word
//! of n letters then takes n trie walks of at most the longest pattern.

use std::collections::HashMap;
use std::sync::OnceLock;

/// The patterns of one language, compiled.
pub struct Patterns {
    nodes: Vec<Node>,
    /// The edges of all nodes: each node's are together, sorted by letter.
    edges: Vec<(char, u32)>,
    /// The digits of all patterns: each one's are together, one more than
    /// its letters.
    values: Vec<u8>,
    /// Words hyphenated in full, where the patterns would get them wrong.
    exceptions: HashMap<String, Vec<usize>>,
    /// Fewest letters before and after a hyphen (TeX's `\lefthyphenmin`
    /// and `\righthyphenmin`).
    pub left: usize,
    pub right: usize,
}

#[derive(Clone, Copy, Default)]
struct Node {
    first_edge: u32,
    edges: u32,
    /// The digits of the pattern that ends here, if one does.
    value: u32,
    value_len: u32,
}

/// Hyphenation patterns for a language, by its BCP 47 tag (`en`, `en-US`),
/// if sundowner has them.
pub fn for_language(tag: &str) -> Option<&'static Patterns> {
    let primary = tag.split(['-', '_']).next()?.to_ascii_lowercase();
    match primary.as_str() {
        // US patterns for every variety of English: British hyphenation
        // differs in places (etymological rather than by pronunciation),
        // but rarely gives a wrong break.
        "en" => {
            static EN: OnceLock<Patterns> = OnceLock::new();
            Some(EN.get_or_init(|| {
                let mut p = Patterns::new(
                    include_str!("../hyph/hyph-en-us.pat.txt"),
                    include_str!("../hyph/hyph-en-us.hyp.txt"),
                    2,
                    3,
                );
                // A known bug of the patterns (hyph-utf8 issue 15).
                p.add_exceptions("dem-o-crat dem-o-crats");
                p
            }))
        }
        _ => None,
    }
}

/// Whether `tag` looks like a BCP 47 language tag: letters, digits and
/// hyphens, starting with a language subtag of letters.
pub fn valid_tag(tag: &str) -> bool {
    let mut parts = tag.split(['-', '_']);
    let first = parts.next().unwrap_or("");
    (2..=8).contains(&first.len())
        && first.chars().all(|c| c.is_ascii_alphabetic())
        && parts.all(|p| (1..=8).contains(&p.len()) && p.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// The licenses of the bundled patterns (shown by `sundowner --licenses`).
pub const LICENSES: &[(&str, &str)] = &[(
    "Hyphenation patterns for English (hyph-en-us)",
    include_str!("../hyph/LICENSE-en-us.txt"),
)];

impl Patterns {
    /// Compile patterns (whitespace-separated, like `.ach4`) and exceptions
    /// (whitespace-separated words, like `ta-ble`).
    pub fn new(patterns: &str, exceptions: &str, left: usize, right: usize) -> Patterns {
        // Build a trie with a map per node, then flatten it.
        let mut children: Vec<Vec<(char, u32)>> = vec![Vec::new()];
        let mut ends: Vec<Option<(u32, u32)>> = vec![None];
        let mut values = Vec::new();
        for pat in patterns.split_whitespace() {
            let mut node = 0usize;
            let mut digits = vec![0u8];
            for c in pat.chars() {
                if let Some(d) = c.to_digit(10) {
                    *digits.last_mut().expect("digits is not empty") = d as u8;
                    continue;
                }
                let c = lower(c);
                node = match children[node].iter().find(|e| e.0 == c) {
                    Some(&(_, next)) => next as usize,
                    None => {
                        children.push(Vec::new());
                        ends.push(None);
                        let next = children.len() - 1;
                        children[node].push((c, next as u32));
                        next
                    }
                };
                digits.push(0);
            }
            if digits.iter().any(|&d| d > 0) {
                ends[node] = Some((values.len() as u32, digits.len() as u32));
                values.extend_from_slice(&digits);
            }
        }
        let mut nodes = Vec::with_capacity(children.len());
        let mut edges = Vec::new();
        for (mut ch, end) in children.into_iter().zip(ends) {
            ch.sort_unstable_by_key(|e| e.0);
            let (value, value_len) = end.unwrap_or((0, 0));
            nodes.push(Node {
                first_edge: edges.len() as u32,
                edges: ch.len() as u32,
                value,
                value_len,
            });
            edges.extend(ch);
        }
        let mut p = Patterns {
            nodes,
            edges,
            values,
            exceptions: HashMap::new(),
            left,
            right,
        };
        p.add_exceptions(exceptions);
        p
    }

    fn add_exceptions(&mut self, words: &str) {
        for w in words.split_whitespace() {
            let mut word = String::new();
            let mut points = Vec::new();
            let mut n = 0;
            for c in w.chars() {
                if c == '-' {
                    points.push(n);
                } else {
                    word.push(lower(c));
                    n += 1;
                }
            }
            self.exceptions.insert(word, points);
        }
    }

    fn child(&self, node: usize, c: char) -> Option<usize> {
        let n = self.nodes[node];
        let edges = &self.edges[n.first_edge as usize..(n.first_edge + n.edges) as usize];
        edges
            .binary_search_by_key(&c, |e| e.0)
            .ok()
            .map(|k| edges[k].1 as usize)
    }

    /// Whether the patterns know the letter `c` (lowercase), that is,
    /// whether they are meant for words written with it.
    pub fn knows(&self, c: char) -> bool {
        c != '.' && self.child(0, c).is_some()
    }

    /// Where the lowercase `word` may be hyphenated: the indices of the
    /// letters a hyphen may come before, at least `left` from the start
    /// and `right` from the end.
    pub fn points(&self, word: &[char]) -> Vec<usize> {
        let n = word.len();
        if n < self.left + self.right {
            return Vec::new();
        }
        if !self.exceptions.is_empty() {
            let key: String = word.iter().collect();
            if let Some(p) = self.exceptions.get(&key) {
                return p.clone();
            }
        }
        let mut w = Vec::with_capacity(n + 2);
        w.push('.');
        w.extend_from_slice(word);
        w.push('.');
        // v[j]: the highest digit between w[j - 1] and w[j].
        let mut v = vec![0u8; w.len() + 1];
        for start in 0..w.len() {
            let mut node = 0;
            for &c in &w[start..] {
                let Some(next) = self.child(node, c) else { break };
                node = next;
                let nd = self.nodes[node];
                let digits = &self.values[nd.value as usize..(nd.value + nd.value_len) as usize];
                for (k, &d) in digits.iter().enumerate() {
                    v[start + k] = v[start + k].max(d);
                }
            }
        }
        // A hyphen before word[p] is between w[p] and w[p + 1].
        (self.left..=n - self.right)
            .filter(|&p| v[p + 1] % 2 == 1)
            .collect()
    }
}

fn lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hyphenate(word: &str) -> String {
        let p = for_language("en-US").unwrap();
        let chars: Vec<char> = word.chars().collect();
        let points = p.points(&chars);
        let mut out = String::new();
        for (i, c) in chars.iter().enumerate() {
            if points.contains(&i) {
                out.push('-');
            }
            out.push(*c);
        }
        out
    }

    #[test]
    fn english_words() {
        for w in [
            "hy-phen-ation",
            "al-go-rithm",
            "com-puter",
            "ta-ble",
            "project",
            "dem-o-crat",
            "sun-downer",
            "type-set-ting",
            "ex-am-ple",
            "jus-ti-fi-ca-tion",
        ] {
            assert_eq!(hyphenate(&w.replace('-', "")), w);
        }
    }

    #[test]
    fn short_words_are_not_hyphenated() {
        for w in ["a", "an", "the", "into", "also"] {
            assert_eq!(hyphenate(w), w);
        }
    }

    #[test]
    fn language_tags() {
        assert!(for_language("en").is_some());
        assert!(for_language("EN-gb").is_some());
        assert!(for_language("en_US").is_some());
        assert!(for_language("sv").is_none());
        assert!(for_language("").is_none());
        assert!(valid_tag("en") && valid_tag("en-US") && valid_tag("zh-Hant-TW") && valid_tag("sr-Latn"));
        assert!(!valid_tag("") && !valid_tag("e") && !valid_tag("en US") && !valid_tag("1a"));
    }
}

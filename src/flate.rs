//! Minimal zlib/DEFLATE (RFC 1950/1951) encoder and decoder.
//!
//! The encoder finds matches by LZ77 with hash chains and lazy matching, and
//! codes each block with Huffman codes built for its symbols, the fixed
//! code, or stores it, whichever is smallest. The decoder handles every
//! block type, decodes most codes with one table lookup, and enforces an
//! output limit so hostile input cannot exhaust memory.

const LBASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195,
    227, 258,
];
const LEXT: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DBASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073,
    4097, 6145, 8193, 12289, 16385, 24577,
];
const DEXT: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13,
];
/// The order in which the lengths of the code length code are sent.
const CL_ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

/// The code lengths of the fixed literal/length code.
fn fixed_lengths() -> [u8; 288] {
    let mut l = [0u8; 288];
    l[..144].fill(8);
    l[144..256].fill(9);
    l[256..280].fill(7);
    l[280..].fill(8);
    l
}

// ---------------------------------------------------------------- encoder

struct BitWriter {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl BitWriter {
    fn put(&mut self, value: u32, bits: u32) {
        debug_assert!(bits <= 32 && self.n + bits <= 64);
        self.acc |= (value as u64) << self.n;
        self.n += bits;
        while self.n >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.n -= 8;
        }
    }

    /// Pad to a byte boundary with zero bits.
    fn align(&mut self) {
        if self.n > 0 {
            self.out.push(self.acc as u8);
            self.acc = 0;
            self.n = 0;
        }
    }

    fn finish(mut self) -> Vec<u8> {
        self.align();
        self.out
    }
}

/// A symbol of the LZ77 parse: a literal byte (`len` 0, `value` the byte),
/// or a match of `len` bytes at distance `value`.
#[derive(Clone, Copy)]
struct Sym {
    len: u16,
    value: u16,
}

/// The length code (0 to 28, less 257) of a match length.
fn len_code(len: u16) -> usize {
    let l = (len - 3) as u32;
    match l {
        255 => 28,
        0..=7 => l as usize,
        _ => {
            let b = 31 - l.leading_zeros();
            (4 * (b - 1) + ((l >> (b - 2)) & 3)) as usize
        }
    }
}

/// The distance code (0 to 29) of a match distance.
fn dist_code(dist: u16) -> usize {
    let d = (dist - 1) as u32;
    if d < 4 {
        return d as usize;
    }
    let b = 31 - d.leading_zeros();
    (2 * b + ((d >> (b - 1)) & 1)) as usize
}

/// Lengths of a Huffman code for symbols with frequencies `freq`, none
/// longer than `max_bits`, by the package-merge algorithm, which gives the
/// optimal lengths under that limit. At least two symbols get a code (the
/// first ones, if fewer are used), so that every decoder accepts it.
fn huffman_lengths(freq: &[u32], max_bits: u32) -> Vec<u8> {
    let mut leaves: Vec<(u64, usize)> = freq
        .iter()
        .enumerate()
        .filter(|(_, &f)| f > 0)
        .map(|(s, &f)| (f as u64, s))
        .collect();
    for s in 0..freq.len().min(2) {
        if leaves.len() < 2 && !leaves.iter().any(|l| l.1 == s) {
            leaves.push((1, s));
        }
    }
    leaves.sort_unstable();
    let n = leaves.len();
    let mut lengths = vec![0u8; freq.len()];
    // Nodes: the leaves are nodes 0..n; packages of two nodes follow.
    let mut weight: Vec<u64> = leaves.iter().map(|l| l.0).collect();
    let mut kids: Vec<(u32, u32)> = vec![(0, 0); n];
    let mut list: Vec<u32> = (0..n as u32).collect();
    for _ in 1..max_bits {
        let mut merged = Vec::with_capacity(n + list.len() / 2);
        let mut leaf = 0;
        for pair in list.chunks_exact(2) {
            let w = weight[pair[0] as usize] + weight[pair[1] as usize];
            while leaf < n && weight[leaf] <= w {
                merged.push(leaf as u32);
                leaf += 1;
            }
            merged.push(weight.len() as u32);
            weight.push(w);
            kids.push((pair[0], pair[1]));
        }
        merged.extend(leaf as u32..n as u32);
        list = merged;
    }
    // A symbol's code length is how often its leaf is in the first 2n - 2
    // items of the last list.
    let mut counts = vec![0u8; n];
    let mut stack = Vec::new();
    for &item in &list[..2 * n - 2] {
        stack.push(item);
        while let Some(x) = stack.pop() {
            match kids.get(x as usize) {
                Some(_) if (x as usize) < n => counts[x as usize] += 1,
                Some(&(a, b)) => {
                    stack.push(a);
                    stack.push(b);
                }
                None => {}
            }
        }
    }
    for (k, &(_, s)) in leaves.iter().enumerate() {
        lengths[s] = counts[k];
    }
    lengths
}

/// The canonical codes for code lengths, bit-reversed, as DEFLATE sends
/// Huffman codes most-significant bit first.
fn canonical_codes(lengths: &[u8]) -> Vec<u16> {
    let mut count = [0u16; 16];
    for &l in lengths {
        count[l as usize] += 1;
    }
    count[0] = 0;
    let mut next = [0u16; 16];
    let mut code = 0u16;
    for b in 1..16 {
        code = (code + count[b - 1]) << 1;
        next[b] = code;
    }
    lengths
        .iter()
        .map(|&l| {
            if l == 0 {
                return 0;
            }
            let c = next[l as usize];
            next[l as usize] += 1;
            c.reverse_bits() >> (16 - l)
        })
        .collect()
}

/// Run-length encode code lengths with the symbols 16 (repeat the last
/// length 3 to 6 times), 17 (3 to 10 zeros) and 18 (11 to 138 zeros), as
/// (symbol, extra bits value) pairs.
fn run_lengths(lengths: &[u8]) -> Vec<(u8, u8)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lengths.len() {
        let v = lengths[i];
        let mut run = lengths[i..].iter().take_while(|&&l| l == v).count();
        i += run;
        if v == 0 {
            while run >= 11 {
                let r = run.min(138);
                out.push((18, (r - 11) as u8));
                run -= r;
            }
            if run >= 3 {
                out.push((17, (run - 3) as u8));
                run = 0;
            }
        } else {
            out.push((v, 0));
            run -= 1;
            while run >= 3 {
                let r = run.min(6);
                out.push((16, (r - 3) as u8));
                run -= r;
            }
        }
        out.extend(std::iter::repeat_n((v, 0), run));
    }
    out
}

/// The header of a dynamic block for these literal/length and distance
/// code lengths: the numbers of codes sent, the code length code's lengths
/// and the run-length encoded lengths, and its size in bits.
struct Header {
    hlit: usize,
    hdist: usize,
    hclen: usize,
    cl_lengths: Vec<u8>,
    runs: Vec<(u8, u8)>,
    bits: u64,
}

impl Header {
    fn new(lit: &[u8], dist: &[u8]) -> Header {
        let hlit = lit.iter().rposition(|&l| l != 0).map_or(0, |p| p + 1).max(257);
        let hdist = dist.iter().rposition(|&l| l != 0).map_or(0, |p| p + 1).max(1);
        let all: Vec<u8> = lit[..hlit].iter().chain(&dist[..hdist]).copied().collect();
        let runs = run_lengths(&all);
        let mut freq = [0u32; 19];
        for &(s, _) in &runs {
            freq[s as usize] += 1;
        }
        let cl_lengths = huffman_lengths(&freq, 7);
        let hclen = CL_ORDER
            .iter()
            .rposition(|&s| cl_lengths[s] != 0)
            .map_or(0, |p| p + 1)
            .max(4);
        let extra = [2u64, 3, 7];
        let bits = 14
            + 3 * hclen as u64
            + runs
                .iter()
                .map(|&(s, _)| {
                    cl_lengths[s as usize] as u64 + extra.get((s as usize).wrapping_sub(16)).unwrap_or(&0)
                })
                .sum::<u64>();
        Header {
            hlit,
            hdist,
            hclen,
            cl_lengths,
            runs,
            bits,
        }
    }

    fn write(&self, w: &mut BitWriter) {
        w.put((self.hlit - 257) as u32, 5);
        w.put((self.hdist - 1) as u32, 5);
        w.put((self.hclen - 4) as u32, 4);
        for &s in &CL_ORDER[..self.hclen] {
            w.put(self.cl_lengths[s] as u32, 3);
        }
        let codes = canonical_codes(&self.cl_lengths);
        for &(s, x) in &self.runs {
            w.put(codes[s as usize] as u32, self.cl_lengths[s as usize] as u32);
            match s {
                16 => w.put(x as u32, 2),
                17 => w.put(x as u32, 3),
                18 => w.put(x as u32, 7),
                _ => {}
            }
        }
    }
}

/// Write the symbols of a block with the given code lengths.
fn write_symbols(w: &mut BitWriter, syms: &[Sym], lit: &[u8], dist: &[u8]) {
    let lcodes = canonical_codes(lit);
    let dcodes = canonical_codes(dist);
    for s in syms {
        if s.len == 0 {
            let v = s.value as usize;
            w.put(lcodes[v] as u32, lit[v] as u32);
            continue;
        }
        let lc = len_code(s.len);
        w.put(lcodes[257 + lc] as u32, lit[257 + lc] as u32);
        w.put((s.len - LBASE[lc]) as u32, LEXT[lc] as u32);
        let dc = dist_code(s.value);
        w.put(dcodes[dc] as u32, dist[dc] as u32);
        w.put((s.value - DBASE[dc]) as u32, DEXT[dc] as u32);
    }
    w.put(lcodes[256] as u32, lit[256] as u32);
}

/// Write one block of symbols, which stand for the bytes `raw`, in
/// whichever form is smallest: with its own codes, the fixed codes, or
/// stored.
fn write_block(w: &mut BitWriter, syms: &[Sym], raw: &[u8], last: bool) {
    let mut lfreq = [0u32; 286];
    let mut dfreq = [0u32; 30];
    for s in syms {
        if s.len == 0 {
            lfreq[s.value as usize] += 1;
        } else {
            lfreq[257 + len_code(s.len)] += 1;
            dfreq[dist_code(s.value)] += 1;
        }
    }
    lfreq[256] = 1;
    let cost = |lit: &[u8], dist: &[u8]| -> u64 {
        let l: u64 = lfreq.iter().zip(lit).map(|(&f, &b)| f as u64 * b as u64).sum();
        let d: u64 = dfreq.iter().zip(dist).map(|(&f, &b)| f as u64 * b as u64).sum();
        l + d
    };
    let extra: u64 = lfreq[257..]
        .iter()
        .zip(LEXT)
        .map(|(&f, x)| f as u64 * x as u64)
        .sum::<u64>()
        + dfreq
            .iter()
            .zip(DEXT)
            .map(|(&f, x)| f as u64 * x as u64)
            .sum::<u64>();
    let lit = huffman_lengths(&lfreq, 15);
    let dist = huffman_lengths(&dfreq, 15);
    let header = Header::new(&lit, &dist);
    let dynamic = 3 + header.bits + cost(&lit, &dist) + extra;
    let fixed_lit = fixed_lengths();
    let fixed_dist = [5u8; 30];
    let fixed = 3 + cost(&fixed_lit, &fixed_dist) + extra;
    // Stored blocks hold at most 65535 bytes, each with a header of 3 bits,
    // padding to a byte and 4 bytes of lengths. The first header starts
    // where the last block ended; the others at a byte boundary.
    let chunks = raw.len().div_ceil(65535).max(1);
    let first_pad = (8 - (w.n + 3) % 8) % 8;
    let stored = 3 + first_pad as u64 + 32 + 8 * raw.len() as u64 + 40 * (chunks as u64 - 1);

    if stored < dynamic.min(fixed) {
        let mut rest = raw;
        for k in 0..chunks {
            let (chunk, tail) = rest.split_at(rest.len().min(65535));
            rest = tail;
            w.put((last && k + 1 == chunks) as u32, 1);
            w.put(0, 2);
            w.align();
            let len = chunk.len() as u16;
            w.out.extend_from_slice(&len.to_le_bytes());
            w.out.extend_from_slice(&(!len).to_le_bytes());
            w.out.extend_from_slice(chunk);
        }
    } else if fixed <= dynamic {
        w.put(last as u32, 1);
        w.put(1, 2);
        write_symbols(w, syms, &fixed_lit, &fixed_dist);
    } else {
        w.put(last as u32, 1);
        w.put(2, 2);
        header.write(w);
        write_symbols(w, syms, &lit, &dist);
    }
}

/// The length of the common prefix of two slices of the same length,
/// compared eight bytes at a time.
fn common_prefix(a: &[u8], b: &[u8]) -> usize {
    let mut l = 0;
    for (x, y) in a.chunks_exact(8).zip(b.chunks_exact(8)) {
        let d = u64::from_le_bytes(x.try_into().expect("8 bytes"))
            ^ u64::from_le_bytes(y.try_into().expect("8 bytes"));
        if d != 0 {
            return l + (d.trailing_zeros() / 8) as usize;
        }
        l += 8;
    }
    l + a[l..].iter().zip(&b[l..]).take_while(|(x, y)| x == y).count()
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in data.chunks(5552) {
        for &x in chunk {
            a += x as u32;
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (b << 16) | a
}

/// Compress `data` into a zlib stream.
pub fn zlib_compress(data: &[u8]) -> Vec<u8> {
    const WSIZE: usize = 32768;
    const HBITS: u32 = 15;
    const MAX_CHAIN: usize = 64;
    /// A match at least this long ends the search for a longer one.
    const NICE: usize = 128;
    /// Having a match at least this long already, look for a longer one at
    /// the next byte along a quarter of the chain.
    const GOOD: usize = 8;
    /// A match at least this long is taken without looking for a longer
    /// one at the next byte.
    const MAX_LAZY: usize = 32;
    /// Matches of 3 bytes further back than this cost more than the
    /// literals.
    const TOO_FAR: usize = 4096;
    /// Symbols per block: blocks adapt their codes to the data.
    const BLOCK_SYMS: usize = 1 << 15;
    const NIL: u32 = u32::MAX;

    let mut w = BitWriter {
        out: Vec::with_capacity(data.len() / 3 + 16),
        acc: 0,
        n: 0,
    };
    w.out.extend_from_slice(&[0x78, 0xDA]);

    let mut head = vec![NIL; 1 << HBITS];
    let mut prev = vec![NIL; WSIZE];
    let hash = |i: usize| -> usize {
        let v = (data[i] as u32) << 16 | (data[i + 1] as u32) << 8 | data[i + 2] as u32;
        (v.wrapping_mul(2654435761) >> (32 - HBITS)) as usize
    };
    let insert = |i: usize, head: &mut Vec<u32>, prev: &mut Vec<u32>| {
        if i + 2 < data.len() {
            let h = hash(i);
            prev[i % WSIZE] = head[h];
            head[h] = i as u32;
        }
    };
    // The longest match for the bytes at `i` among earlier ones, as
    // (length, distance), looking at `max_chain` of them; a length below 3
    // means none.
    let longest = |i: usize, max_chain: usize, head: &Vec<u32>, prev: &Vec<u32>| -> (usize, usize) {
        let (mut best_len, mut best_dist) = (0, 0);
        if i + 2 >= data.len() {
            return (0, 0);
        }
        let mut cand = head[hash(i)];
        let max_len = (data.len() - i).min(258);
        let mut chain = 0;
        while cand != NIL && chain < max_chain {
            let c = cand as usize;
            if c >= i || i - c > WSIZE - 1 {
                break;
            }
            if data[c + best_len.min(max_len - 1)] == data[i + best_len.min(max_len - 1)] {
                let l = common_prefix(&data[c..c + max_len], &data[i..i + max_len]);
                if l > best_len {
                    best_len = l;
                    best_dist = i - c;
                    if l == max_len || l >= NICE {
                        break;
                    }
                }
            }
            let next = prev[c % WSIZE];
            if next != NIL && next as usize >= c {
                break;
            }
            cand = next;
            chain += 1;
        }
        if best_len == 3 && best_dist > TOO_FAR {
            return (0, 0);
        }
        (best_len, best_dist)
    };

    let mut syms: Vec<Sym> = Vec::with_capacity(BLOCK_SYMS);
    // Where the current block's bytes start, and how many its symbols cover.
    let (mut block_start, mut covered) = (0usize, 0usize);
    let mut emit = |w: &mut BitWriter, s: Sym, syms: &mut Vec<Sym>| {
        syms.push(s);
        covered += if s.len == 0 { 1 } else { s.len as usize };
        if syms.len() == BLOCK_SYMS {
            write_block(w, syms, &data[block_start..block_start + covered], false);
            syms.clear();
            block_start += covered;
            covered = 0;
        }
    };
    let lit = |i: usize| Sym {
        len: 0,
        value: data[i] as u16,
    };
    let matched = |(len, dist): (usize, usize)| Sym {
        len: len as u16,
        value: dist as u16,
    };

    // Lazy matching: a match found at one byte is held back while the next
    // byte is tried; if that one starts a longer match, the first byte goes
    // out as a literal instead.
    let mut pending: Option<(usize, usize)> = None;
    let mut i = 0;
    while i < data.len() {
        let found = match pending {
            Some((len, _)) if len >= MAX_LAZY => (0, 0),
            Some((len, _)) if len >= GOOD => longest(i, MAX_CHAIN / 4, &head, &prev),
            _ => longest(i, MAX_CHAIN, &head, &prev),
        };
        match pending.take() {
            Some(held) if found.0 <= held.0 => {
                // The held match starts at i - 1; i - 1 is in the hash
                // chains already, and so is i from here.
                emit(&mut w, matched(held), &mut syms);
                insert(i, &mut head, &mut prev);
                for k in i + 1..i - 1 + held.0 {
                    insert(k, &mut head, &mut prev);
                }
                i = i - 1 + held.0;
            }
            Some(_) => {
                emit(&mut w, lit(i - 1), &mut syms);
                pending = Some(found);
                insert(i, &mut head, &mut prev);
                i += 1;
            }
            None if found.0 >= 3 => {
                pending = Some(found);
                insert(i, &mut head, &mut prev);
                i += 1;
            }
            None => {
                emit(&mut w, lit(i), &mut syms);
                insert(i, &mut head, &mut prev);
                i += 1;
            }
        }
    }
    if let Some(held) = pending {
        emit(&mut w, matched(held), &mut syms);
    }
    write_block(&mut w, &syms, &data[block_start..block_start + covered], true);
    let mut out = w.finish();
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

// ---------------------------------------------------------------- decoder

pub type Error = &'static str;

/// Reads bits least-significant first, holding up to 64 of them ahead.
/// Past the end of the input it reads zeros, but consuming them fails.
struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    acc: u64,
    n: u32,
}

impl BitReader<'_> {
    fn refill(&mut self) {
        if let Some(bytes) = self.data.get(self.pos..self.pos + 8) {
            let take = (64 - self.n) / 8;
            let v = u64::from_le_bytes(bytes.try_into().expect("8 bytes"));
            let v = if take == 8 {
                v
            } else {
                v & ((1u64 << (take * 8)) - 1)
            };
            self.acc |= v << self.n;
            self.pos += take as usize;
            self.n += take * 8;
            return;
        }
        while self.n <= 56 {
            let Some(&b) = self.data.get(self.pos) else { break };
            self.acc |= (b as u64) << self.n;
            self.pos += 1;
            self.n += 8;
        }
    }

    /// The next `k` bits (at most 32), without consuming them.
    fn peek(&mut self, k: u32) -> u32 {
        if self.n < k {
            self.refill();
        }
        (self.acc & ((1u64 << k) - 1)) as u32
    }

    fn consume(&mut self, k: u32) -> Result<(), Error> {
        if k > self.n {
            return Err("truncated deflate stream");
        }
        self.acc >>= k;
        self.n -= k;
        Ok(())
    }

    fn bits(&mut self, k: u32) -> Result<u32, Error> {
        let v = self.peek(k);
        self.consume(k)?;
        Ok(v)
    }
}

/// Codes up to this long are decoded with one table lookup.
const FAST_BITS: u32 = 10;

struct Huffman {
    /// For each value of the next `FAST_BITS` bits, the symbol whose code
    /// they start with and the code's length (`symbol << 4 | length`), or 0
    /// if the code is longer.
    fast: Vec<u16>,
    counts: [u16; 16],
    symbols: Vec<u16>,
}

impl Huffman {
    fn new(lengths: &[u8]) -> Result<Huffman, Error> {
        if lengths.iter().any(|&l| l > 15) {
            return Err("bad code length");
        }
        let mut counts = [0u16; 16];
        for &l in lengths {
            counts[l as usize] += 1;
        }
        counts[0] = 0;
        let mut left: i32 = 1;
        for &c in &counts[1..] {
            left = left * 2 - c as i32;
            if left < 0 {
                return Err("over-subscribed huffman code");
            }
        }
        let mut offs = [0u16; 16];
        for l in 1..15 {
            offs[l + 1] = offs[l] + counts[l];
        }
        let mut symbols = vec![0u16; lengths.len()];
        for (sym, &l) in lengths.iter().enumerate() {
            if l != 0 {
                let o = &mut offs[l as usize];
                symbols[*o as usize] = sym as u16;
                *o += 1;
            }
        }
        let mut fast = vec![0u16; 1 << FAST_BITS];
        for (sym, (&l, code)) in lengths.iter().zip(canonical_codes(lengths)).enumerate() {
            if l == 0 || l as u32 > FAST_BITS {
                continue;
            }
            for idx in (code as usize..1 << FAST_BITS).step_by(1 << l) {
                fast[idx] = (sym as u16) << 4 | l as u16;
            }
        }
        Ok(Huffman {
            fast,
            counts,
            symbols,
        })
    }

    fn decode(&self, r: &mut BitReader) -> Result<u16, Error> {
        let e = self.fast[r.peek(FAST_BITS) as usize];
        if e != 0 {
            r.consume(e as u32 & 15)?;
            return Ok(e >> 4);
        }
        // A longer code: walk the canonical code one bit at a time.
        let bits = r.peek(15);
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for len in 1..16 {
            code |= (bits >> (len - 1)) as i32 & 1;
            let count = self.counts[len as usize] as i32;
            if code - count < first {
                r.consume(len)?;
                let idx = (index + code - first) as usize;
                return self.symbols.get(idx).copied().ok_or("bad huffman code");
            }
            index += count;
            first += count;
            first <<= 1;
            code <<= 1;
        }
        Err("bad huffman code")
    }
}

fn inflate_block(
    r: &mut BitReader,
    out: &mut Vec<u8>,
    lit: &Huffman,
    dist: &Huffman,
    limit: usize,
) -> Result<(), Error> {
    loop {
        let sym = lit.decode(r)? as usize;
        if sym < 256 {
            if out.len() >= limit {
                return Err("decompressed data too large");
            }
            out.push(sym as u8);
        } else if sym == 256 {
            return Ok(());
        } else {
            let li = sym - 257;
            if li >= 29 {
                return Err("bad length code");
            }
            let len = LBASE[li] as usize + r.bits(LEXT[li] as u32)? as usize;
            let di = dist.decode(r)? as usize;
            if di >= 30 {
                return Err("bad distance code");
            }
            let d = DBASE[di] as usize + r.bits(DEXT[di] as u32)? as usize;
            if d > out.len() {
                return Err("distance too far back");
            }
            if out.len() + len > limit {
                return Err("decompressed data too large");
            }
            let start = out.len() - d;
            if d >= len {
                out.extend_from_within(start..start + len);
            } else if d == 1 {
                let b = out[start];
                out.resize(out.len() + len, b);
            } else {
                for k in 0..len {
                    let b = out[start + k];
                    out.push(b);
                }
            }
        }
    }
}

/// Decompress a zlib stream, failing if the output would exceed `limit` bytes.
pub fn zlib_decompress(data: &[u8], limit: usize) -> Result<Vec<u8>, Error> {
    if data.len() < 2 {
        return Err("truncated zlib header");
    }
    let (cmf, flg) = (data[0], data[1]);
    if cmf & 0x0F != 8 || !((cmf as u16) << 8 | flg as u16).is_multiple_of(31) || flg & 0x20 != 0 {
        return Err("bad zlib header");
    }
    let mut r = BitReader {
        data: &data[2..],
        pos: 0,
        acc: 0,
        n: 0,
    };
    let mut out = Vec::new();
    loop {
        let last = r.bits(1)?;
        match r.bits(2)? {
            0 => {
                // Skip to a byte boundary, and give back the whole bytes
                // read ahead.
                r.consume(r.n % 8)?;
                r.pos -= (r.n / 8) as usize;
                r.acc = 0;
                r.n = 0;
                let p = r.pos;
                let hdr = r.data.get(p..p + 4).ok_or("truncated stored block")?;
                let len = u16::from_le_bytes([hdr[0], hdr[1]]) as usize;
                if len != !u16::from_le_bytes([hdr[2], hdr[3]]) as usize {
                    return Err("bad stored block length");
                }
                let bytes = r.data.get(p + 4..p + 4 + len).ok_or("truncated stored block")?;
                if out.len() + len > limit {
                    return Err("decompressed data too large");
                }
                out.extend_from_slice(bytes);
                r.pos = p + 4 + len;
            }
            1 => {
                let lit = Huffman::new(&fixed_lengths())?;
                let dist = Huffman::new(&[5u8; 30])?;
                inflate_block(&mut r, &mut out, &lit, &dist, limit)?;
            }
            2 => {
                let nlen = r.bits(5)? as usize + 257;
                let ndist = r.bits(5)? as usize + 1;
                let ncode = r.bits(4)? as usize + 4;
                if nlen > 286 || ndist > 30 {
                    return Err("bad dynamic block counts");
                }
                let mut cl = [0u8; 19];
                for &o in CL_ORDER.iter().take(ncode) {
                    cl[o] = r.bits(3)? as u8;
                }
                let clh = Huffman::new(&cl)?;
                let mut lengths = vec![0u8; nlen + ndist];
                let mut i = 0;
                while i < nlen + ndist {
                    let sym = clh.decode(&mut r)?;
                    let (val, rep) = match sym {
                        0..=15 => (sym as u8, 1),
                        16 => {
                            let p = *lengths
                                .get(i.wrapping_sub(1))
                                .ok_or("repeat with no previous length")?;
                            (p, 3 + r.bits(2)? as usize)
                        }
                        17 => (0, 3 + r.bits(3)? as usize),
                        _ => (0, 11 + r.bits(7)? as usize),
                    };
                    if i + rep > nlen + ndist {
                        return Err("too many code lengths");
                    }
                    lengths[i..i + rep].fill(val);
                    i += rep;
                }
                if lengths[256] == 0 {
                    return Err("missing end-of-block code");
                }
                let lit = Huffman::new(&lengths[..nlen])?;
                let dist = Huffman::new(&lengths[nlen..])?;
                inflate_block(&mut r, &mut out, &lit, &dist, limit)?;
            }
            _ => return Err("bad block type"),
        }
        if last == 1 {
            // The Adler-32 checksum follows, from the next byte boundary.
            r.consume(r.n % 8)?;
            let p = r.pos - (r.n / 8) as usize;
            let sum = r.data.get(p..p + 4).ok_or("truncated zlib checksum")?;
            if u32::from_be_bytes(sum.try_into().expect("4 bytes")) != adler32(&out) {
                return Err("zlib checksum mismatch");
            }
            return Ok(out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rng(seed: &mut u64) -> u64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        *seed
    }

    #[test]
    fn roundtrip() {
        let mut seed = 0x1234_5678_9abc_def0u64;
        for size in [0usize, 1, 2, 3, 10, 300, 5000, 70000, 200_000] {
            for alphabet in [2u64, 16, 256] {
                let data: Vec<u8> = (0..size).map(|_| (rng(&mut seed) % alphabet) as u8).collect();
                let z = zlib_compress(&data);
                assert_eq!(zlib_decompress(&z, usize::MAX).unwrap(), data);
            }
        }
        let text = b"BT /F1 11 Tf 1 0 0 1 72 700 Tm (Hello world) Tj ET\n".repeat(200);
        let z = zlib_compress(&text);
        assert!(z.len() < text.len() / 10);
        assert_eq!(zlib_decompress(&z, usize::MAX).unwrap(), text);
    }

    #[test]
    fn length_and_distance_codes() {
        for len in 3..=258u16 {
            let c = len_code(len);
            assert!(LBASE[c] <= len && len - LBASE[c] < 1 << LEXT[c], "{len}");
        }
        assert_eq!(len_code(258), 28);
        for dist in 1..=32768u32 {
            let c = dist_code(dist as u16);
            assert!(
                DBASE[c] as u32 <= dist && dist - (DBASE[c] as u32) < 1 << DEXT[c],
                "{dist}"
            );
        }
    }

    #[test]
    fn huffman_lengths_are_limited_and_complete() {
        // Fibonacci frequencies make the unlimited code as deep as possible.
        let mut freq = vec![1u32, 1];
        while freq.len() < 30 {
            let k = freq.len();
            freq.push(freq[k - 1] + freq[k - 2]);
        }
        for max in [7, 9, 15] {
            let lengths = huffman_lengths(&freq, max);
            assert!(lengths.iter().all(|&l| l >= 1 && l as u32 <= max));
            let kraft: f64 = lengths.iter().map(|&l| 0.5f64.powi(l as i32)).sum();
            assert!((kraft - 1.0).abs() < 1e-12, "{max}: {kraft}");
        }
        // Fewer than two symbols used: two codes of one bit.
        assert_eq!(huffman_lengths(&[0, 0, 5], 15), [1, 0, 1]);
        assert_eq!(huffman_lengths(&[0, 0, 0], 15), [1, 1, 0]);
    }

    /// The type of the first block of a zlib stream.
    fn first_block_type(z: &[u8]) -> u8 {
        (z[2] >> 1) & 3
    }

    #[test]
    fn chooses_block_types() {
        let mut seed = 7u64;
        let noise: Vec<u8> = (0..5000).map(|_| rng(&mut seed) as u8).collect();
        let z = zlib_compress(&noise);
        assert_eq!(first_block_type(&z), 0);
        assert!(z.len() < noise.len() + 16);
        assert_eq!(zlib_decompress(&z, usize::MAX).unwrap(), noise);
        assert_eq!(first_block_type(&zlib_compress(b"abc")), 1);
        let text = b"BT /F1 11 Tf 1 0 0 1 72 700 Tm (Hello world) Tj ET\n".repeat(200);
        assert_eq!(first_block_type(&zlib_compress(&text)), 2);
    }

    #[test]
    fn decodes_dynamic_block_from_zlib() {
        // zlib.compress(SAMPLE * 4, 9) from CPython; uses a dynamic Huffman block.
        let z = [
            120, 218, 237, 141, 201, 21, 64, 64, 16, 5, 239, 162, 248, 18, 240, 236, 75, 22, 14, 18, 176, 12,
            198, 214, 102, 24, 91, 244, 58, 3, 9, 56, 87, 213, 171, 162, 23, 80, 70, 214, 35, 42, 77, 231,
            130, 150, 46, 12, 102, 94, 55, 208, 33, 52, 118, 198, 83, 249, 220, 104, 168, 115, 144, 151, 236,
            205, 55, 42, 150, 78, 185, 247, 104, 229, 33, 24, 61, 98, 193, 36, 149, 33, 205, 109, 183, 217,
            112, 61, 63, 8, 163, 56, 73, 51, 171, 248, 7, 95, 131, 23, 116, 203, 131, 161,
        ];
        let expected = b"The quick brown fox jumps over the lazy dog. Pack my box with five dozen liquor jugs! 0123456789\n".repeat(4);
        match zlib_decompress(&z, 1 << 20) {
            Ok(v) => assert_eq!(v, expected),
            Err(e) => panic!("{e}"),
        }
    }

    #[test]
    fn garbage_never_panics() {
        let mut seed = 42u64;
        for _ in 0..2000 {
            let len = (rng(&mut seed) % 64) as usize;
            let mut data: Vec<u8> = (0..len).map(|_| rng(&mut seed) as u8).collect();
            if data.len() >= 2 {
                data[0] = 0x78;
                data[1] = 0x9C;
            }
            let _ = zlib_decompress(&data, 1 << 16);
        }
        // Valid streams with a few bytes changed or cut short.
        let text = b"The quick brown fox jumps over the lazy dog. 0123456789\n".repeat(50);
        let z = zlib_compress(&text);
        for _ in 0..2000 {
            let mut data = z.clone();
            for _ in 0..3 {
                let k = 2 + (rng(&mut seed) as usize) % (data.len() - 2);
                data[k] = rng(&mut seed) as u8;
            }
            data.truncate(2 + (rng(&mut seed) as usize) % (z.len() - 1));
            let _ = zlib_decompress(&data, 1 << 16);
        }
    }

    #[test]
    fn rejects_bad_lengths_and_checksums() {
        assert!(Huffman::new(&[1, 16]).is_err());
        let mut z = zlib_compress(b"hello hello hello");
        assert!(zlib_decompress(&z, 1 << 10).is_ok());
        let n = z.len();
        z[n - 1] ^= 1;
        assert_eq!(zlib_decompress(&z, 1 << 10), Err("zlib checksum mismatch"));
        assert!(zlib_decompress(&z[..n - 2], 1 << 10).is_err());
    }

    #[test]
    fn output_limit_enforced() {
        let z = zlib_compress(&vec![0u8; 100_000]);
        assert!(zlib_decompress(&z, 1000).is_err());
    }
}

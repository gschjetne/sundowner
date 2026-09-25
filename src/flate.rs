//! Minimal zlib/DEFLATE (RFC 1950/1951) encoder and decoder.
//!
//! The encoder uses LZ77 with hash chains and the fixed Huffman code, which is
//! simple and gives good ratios on PDF content streams. The decoder handles
//! every block type and enforces an output limit so hostile input cannot
//! exhaust memory.

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

// ---------------------------------------------------------------- encoder

struct BitWriter {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl BitWriter {
    fn put(&mut self, value: u32, bits: u32) {
        self.acc |= (value as u64) << self.n;
        self.n += bits;
        while self.n >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.n -= 8;
        }
    }

    /// Write a Huffman code, which is defined most-significant bit first.
    fn put_code(&mut self, code: u32, len: u32) {
        let rev = code.reverse_bits() >> (32 - len);
        self.put(rev, len);
    }

    fn finish(mut self) -> Vec<u8> {
        if self.n > 0 {
            self.out.push(self.acc as u8);
        }
        self.out
    }
}

fn put_literal(w: &mut BitWriter, sym: u32) {
    match sym {
        0..=143 => w.put_code(0x30 + sym, 8),
        144..=255 => w.put_code(0x190 + sym - 144, 9),
        256..=279 => w.put_code(sym - 256, 7),
        _ => w.put_code(0xC0 + sym - 280, 8),
    }
}

fn put_match(w: &mut BitWriter, len: usize, dist: usize) {
    let li = LBASE.iter().rposition(|&b| b as usize <= len).unwrap_or(0);
    put_literal(w, 257 + li as u32);
    w.put((len - LBASE[li] as usize) as u32, LEXT[li] as u32);
    let di = DBASE.iter().rposition(|&b| b as usize <= dist).unwrap_or(0);
    w.put_code(di as u32, 5);
    w.put((dist - DBASE[di] as usize) as u32, DEXT[di] as u32);
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
    const MAX_CHAIN: usize = 48;
    const NIL: u32 = u32::MAX;

    let mut w = BitWriter {
        out: Vec::with_capacity(data.len() / 3 + 16),
        acc: 0,
        n: 0,
    };
    w.out.extend_from_slice(&[0x78, 0x9C]);
    w.put(1, 1); // BFINAL
    w.put(1, 2); // fixed Huffman

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

    let mut i = 0;
    while i < data.len() {
        let mut best_len = 0;
        let mut best_dist = 0;
        if i + 2 < data.len() {
            let mut cand = head[hash(i)];
            let max_len = (data.len() - i).min(258);
            let mut chain = 0;
            while cand != NIL && chain < MAX_CHAIN {
                let c = cand as usize;
                if c >= i || i - c > WSIZE - 1 {
                    break;
                }
                if data[c + best_len.min(max_len - 1)] == data[i + best_len.min(max_len - 1)] {
                    let mut l = 0;
                    while l < max_len && data[c + l] == data[i + l] {
                        l += 1;
                    }
                    if l > best_len {
                        best_len = l;
                        best_dist = i - c;
                        if l == max_len {
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
        }
        if best_len >= 3 {
            put_match(&mut w, best_len, best_dist);
            for k in i..i + best_len {
                insert(k, &mut head, &mut prev);
            }
            i += best_len;
        } else {
            put_literal(&mut w, data[i] as u32);
            insert(i, &mut head, &mut prev);
            i += 1;
        }
    }
    put_literal(&mut w, 256);
    let mut out = w.finish();
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

// ---------------------------------------------------------------- decoder

pub type Error = &'static str;

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    acc: u64,
    n: u32,
}

impl BitReader<'_> {
    fn bits(&mut self, need: u32) -> Result<u32, Error> {
        while self.n < need {
            let b = *self.data.get(self.pos).ok_or("truncated deflate stream")?;
            self.pos += 1;
            self.acc |= (b as u64) << self.n;
            self.n += 8;
        }
        let v = (self.acc & ((1u64 << need) - 1)) as u32;
        self.acc >>= need;
        self.n -= need;
        Ok(v)
    }
}

struct Huffman {
    counts: [u16; 16],
    symbols: Vec<u16>,
}

impl Huffman {
    fn new(lengths: &[u8]) -> Result<Huffman, Error> {
        let mut counts = [0u16; 16];
        for &l in lengths {
            counts[l as usize & 15] += 1;
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
                let o = &mut offs[l as usize & 15];
                symbols[*o as usize] = sym as u16;
                *o += 1;
            }
        }
        Ok(Huffman { counts, symbols })
    }

    fn decode(&self, r: &mut BitReader) -> Result<u16, Error> {
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for len in 1..16 {
            code |= r.bits(1)? as i32;
            let count = self.counts[len] as i32;
            if code - count < first {
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
            for k in 0..len {
                let b = out[start + k];
                out.push(b);
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
                let mut l = [0u8; 288];
                l[..144].fill(8);
                l[144..256].fill(9);
                l[256..280].fill(7);
                l[280..].fill(8);
                let lit = Huffman::new(&l)?;
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
                const ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];
                let mut cl = [0u8; 19];
                for &o in ORDER.iter().take(ncode) {
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
        for size in [0usize, 1, 2, 3, 10, 300, 5000, 70000] {
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
    }

    #[test]
    fn output_limit_enforced() {
        let z = zlib_compress(&vec![0u8; 100_000]);
        assert!(zlib_decompress(&z, 1000).is_err());
    }
}

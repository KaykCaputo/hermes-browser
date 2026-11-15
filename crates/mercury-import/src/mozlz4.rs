// Adapted from Zephium, https://github.com/zephium-browser/Zephium, commit
// 9026a1fffad74d6e201ef97fe8c02c1b01403cd2, crates/zephium-import/src/mozlz4.rs (lines 1-110),
// licensed under MPL-2.0. Made public and documented; the decoder and its tests are unchanged.

//! Mozilla's `mozLz40` files (`.jsonlz4`, `.mozlz4`): an eight-byte magic, the decoded size as
//! a little-endian u32, then one LZ4 block. Firefox keeps its bookmark backups
//! (`bookmarkbackups/*.jsonlz4`) and session files in this format. Decoding is bounded by that
//! size, which is itself bounded, so a damaged file cannot grow without limit.

/// Larger than any real file of this kind, small enough to hold in memory.
pub const MAX_DECODED: usize = 64 * 1024 * 1024;

const MAGIC: &[u8] = b"mozLz40\0";

/// The decoded contents, or `None` if `bytes` is not a complete, undamaged `mozLz40` file of at
/// most [`MAX_DECODED`] bytes.
pub fn decode(bytes: &[u8]) -> Option<Vec<u8>> {
    let body = bytes.strip_prefix(MAGIC)?;
    let size = usize::try_from(u32::from_le_bytes(body.get(..4)?.try_into().ok()?)).ok()?;
    if size > MAX_DECODED {
        return None;
    }
    block(body.get(4..)?, size)
}

fn block(input: &[u8], size: usize) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(size);
    let mut at = 0;
    while at < input.len() {
        let token = input[at];
        at += 1;
        let mut literals = usize::from(token >> 4);
        if literals == 15 {
            literals = literals.checked_add(length(input, &mut at)?)?;
        }
        let end = at.checked_add(literals)?;
        if out.len().checked_add(literals)? > size {
            return None;
        }
        out.extend_from_slice(input.get(at..end)?);
        at = end;
        // The last sequence is literals alone.
        if at == input.len() {
            break;
        }
        let offset = usize::from(u16::from_le_bytes([*input.get(at)?, *input.get(at + 1)?]));
        at += 2;
        if offset == 0 || offset > out.len() {
            return None;
        }
        let mut matched = usize::from(token & 15);
        if matched == 15 {
            matched = matched.checked_add(length(input, &mut at)?)?;
        }
        matched = matched.checked_add(4)?;
        if out.len().checked_add(matched)? > size {
            return None;
        }
        // A match may overlap what it copies, so it goes a byte at a time.
        let start = out.len() - offset;
        for index in 0..matched {
            let byte = out[start + index];
            out.push(byte);
        }
    }
    (out.len() == size).then_some(out)
}

fn length(input: &[u8], at: &mut usize) -> Option<usize> {
    let mut total = 0usize;
    loop {
        let byte = *input.get(*at)?;
        *at += 1;
        total = total.checked_add(usize::from(byte))?;
        if byte != 255 {
            return Some(total);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(size: u32, block: &[u8]) -> Vec<u8> {
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(&size.to_le_bytes());
        bytes.extend_from_slice(block);
        bytes
    }

    #[test]
    fn literals_and_overlapping_matches_decode() {
        // "abcd", then eight bytes from four back, then "e".
        let block = [0x44, b'a', b'b', b'c', b'd', 4, 0, 0x10, b'e'];
        assert_eq!(decode(&file(13, &block)).unwrap(), b"abcdabcdabcde");
        // A run: one byte copied over itself.
        let run = [0x1f, b'x', 1, 0, 6, 0x10, b'y'];
        assert_eq!(
            decode(&file(27, &run)).unwrap(),
            [vec![b'x'; 26], vec![b'y']].concat()
        );
    }

    #[test]
    fn damaged_files_are_refused() {
        let block = [0x44, b'a', b'b', b'c', b'd', 4, 0, 0x10, b'e'];
        assert!(decode(&file(12, &block)).is_none(), "longer than it says");
        assert!(decode(&file(14, &block)).is_none(), "shorter than it says");
        assert!(decode(&file(13, &[0x44, b'a', b'b', b'c', b'd', 9, 0, 0x10, b'e'])).is_none());
        assert!(
            decode(&file(u32::MAX, &block)).is_none(),
            "beyond the bound"
        );
        assert!(decode(b"jsonlz4\0\0\0\0\0").is_none(), "not this format");
    }
}

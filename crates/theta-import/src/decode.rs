//! Byte stream to UTF-8 text, a chunk at a time.
//!
//! Import files come from spreadsheets and exports of every vintage, so the encoding is not a given.
//! `Auto` looks at the first chunk only (a byte-order mark, then NUL-byte patterns that give UTF-16 away,
//! then whether the bytes are valid UTF-8) and commits to the answer: a decoder that re-guessed mid-file
//! would make rows near the switch unreproducible. Nothing here ever holds more than one chunk.

use encoding_rs::{Decoder, Encoding as Enc, UTF_8, UTF_16BE, UTF_16LE, WINDOWS_1252};
use std::io::{self, Read};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Encoding {
    #[default]
    Auto,
    Utf8,
    Utf16Le,
    Utf16Be,
    /// Windows-1252, the WHATWG meaning of "Latin-1" (a superset of ISO-8859-1 in 0x80..0x9F).
    Latin1,
}

impl Encoding {
    pub const ALL: [Encoding; 5] =
        [Encoding::Auto, Encoding::Utf8, Encoding::Utf16Le, Encoding::Utf16Be, Encoding::Latin1];

    pub fn label(self) -> &'static str {
        match self {
            Encoding::Auto => "Auto-detect",
            Encoding::Utf8 => "UTF-8",
            Encoding::Utf16Le => "UTF-16 LE",
            Encoding::Utf16Be => "UTF-16 BE",
            Encoding::Latin1 => "Latin-1",
        }
    }

    fn enc(self) -> Option<&'static Enc> {
        match self {
            Encoding::Auto => None,
            Encoding::Utf8 => Some(UTF_8),
            Encoding::Utf16Le => Some(UTF_16LE),
            Encoding::Utf16Be => Some(UTF_16BE),
            Encoding::Latin1 => Some(WINDOWS_1252),
        }
    }

    fn from_enc(e: &'static Enc) -> Encoding {
        if e == UTF_16LE {
            Encoding::Utf16Le
        } else if e == UTF_16BE {
            Encoding::Utf16Be
        } else if e == WINDOWS_1252 {
            Encoding::Latin1
        } else {
            Encoding::Utf8
        }
    }
}

const CHUNK: usize = 64 * 1024;

/// Guess from the leading bytes. Returns the encoding and the length of a byte-order mark to skip.
pub fn detect(head: &[u8]) -> (Encoding, usize) {
    if let Some((enc, len)) = Enc::for_bom(head) {
        return (Encoding::from_enc(enc), len);
    }
    // ASCII-heavy UTF-16 has a NUL in every other byte; text of any other encoding has none.
    let sample = &head[..head.len().min(512)];
    if sample.len() >= 4 {
        let even = sample.iter().step_by(2).filter(|&&b| b == 0).count();
        let odd = sample.iter().skip(1).step_by(2).filter(|&&b| b == 0).count();
        let half = sample.len() / 2;
        if odd * 10 >= half * 7 && even == 0 {
            return (Encoding::Utf16Le, 0);
        }
        if even * 10 >= half * 7 && odd == 0 {
            return (Encoding::Utf16Be, 0);
        }
    }
    match std::str::from_utf8(head) {
        Ok(_) => (Encoding::Utf8, 0),
        // A chunk boundary may cut a multi-byte character; only a failure before the tail counts.
        Err(e) if e.error_len().is_none() => (Encoding::Utf8, 0),
        Err(_) => (Encoding::Latin1, 0),
    }
}

/// Decodes any `Read` into UTF-8 chunks and counts the bytes it consumed (for progress bars).
pub struct TextChunks<R: Read> {
    inner: R,
    requested: Encoding,
    decoder: Option<Decoder>,
    detected: Encoding,
    raw: Vec<u8>,
    consumed: u64,
    eof: bool,
    /// The decoder had to substitute U+FFFD for malformed input at least once.
    pub had_errors: bool,
}

impl<R: Read> TextChunks<R> {
    pub fn new(inner: R, encoding: Encoding) -> Self {
        Self {
            inner,
            requested: encoding,
            decoder: None,
            detected: encoding,
            raw: vec![0; CHUNK],
            consumed: 0,
            eof: false,
            had_errors: false,
        }
    }

    /// The encoding in use; for `Auto` it is only meaningful after the first chunk was read.
    pub fn encoding(&self) -> Encoding {
        self.detected
    }

    pub fn bytes_consumed(&self) -> u64 {
        self.consumed
    }

    /// Replace `out` with the next chunk of text. `Ok(false)` at the end of input.
    pub fn read_chunk(&mut self, out: &mut String) -> io::Result<bool> {
        out.clear();
        while out.is_empty() {
            if self.eof {
                return Ok(false);
            }
            let n = read_retry(&mut self.inner, &mut self.raw)?;
            self.consumed += n as u64;
            if self.decoder.is_none() {
                // The first read decides the encoding; short reads from pipes are topped up so the
                // detection sees enough bytes to be reliable.
                let mut head = self.raw[..n].to_vec();
                while head.len() < 4096 && n > 0 {
                    let m = read_retry(&mut self.inner, &mut self.raw)?;
                    self.consumed += m as u64;
                    if m == 0 {
                        break;
                    }
                    head.extend_from_slice(&self.raw[..m]);
                }
                let (enc, skip) = match self.requested.enc() {
                    Some(e) => {
                        // An explicit choice still drops a BOM of the same family so it never lands in
                        // the first header cell.
                        let skip = Enc::for_bom(&head).filter(|(b, _)| *b == e).map_or(0, |(_, l)| l);
                        (self.requested, skip)
                    }
                    None => detect(&head),
                };
                self.detected = enc;
                self.decoder = enc.enc().map(|e| e.new_decoder_without_bom_handling());
                if n == 0 {
                    self.eof = true;
                }
                let body = head[skip.min(head.len())..].to_vec();
                self.decode(&body, self.eof, out);
                continue;
            }
            if n == 0 {
                self.eof = true;
            }
            let raw = std::mem::take(&mut self.raw);
            self.decode(&raw[..n], self.eof, out);
            self.raw = raw;
        }
        Ok(true)
    }

    fn decode(&mut self, src: &[u8], last: bool, out: &mut String) {
        let Some(dec) = self.decoder.as_mut() else { return };
        let need = dec.max_utf8_buffer_length(src.len()).unwrap_or(src.len() * 3 + 16);
        out.reserve(need);
        let (_, _, errs) = dec.decode_to_string(src, out, last);
        self.had_errors |= errs;
    }
}

fn read_retry<R: Read>(r: &mut R, buf: &mut [u8]) -> io::Result<usize> {
    loop {
        match r.read(buf) {
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            other => return other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(bytes: &[u8], enc: Encoding) -> (String, Encoding) {
        let mut t = TextChunks::new(bytes, enc);
        let (mut s, mut chunk) = (String::new(), String::new());
        while t.read_chunk(&mut chunk).unwrap() {
            s.push_str(&chunk);
        }
        (s, t.encoding())
    }

    #[test]
    fn utf8_bom_is_stripped() {
        let (s, e) = all(b"\xEF\xBB\xBFa,b\n1,2\n", Encoding::Auto);
        assert_eq!((s.as_str(), e), ("a,b\n1,2\n", Encoding::Utf8));
    }

    #[test]
    fn utf16_with_and_without_bom() {
        let text = "id,näme\n1,é\n";
        let mut le = vec![0xFF, 0xFE];
        le.extend(text.encode_utf16().flat_map(|u| u.to_le_bytes()));
        assert_eq!(all(&le, Encoding::Auto), (text.to_string(), Encoding::Utf16Le));
        let be: Vec<u8> = text.encode_utf16().flat_map(|u| u.to_be_bytes()).collect();
        assert_eq!(all(&be, Encoding::Auto), (text.to_string(), Encoding::Utf16Be));
        let le_nobom: Vec<u8> = text.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        assert_eq!(all(&le_nobom, Encoding::Auto), (text.to_string(), Encoding::Utf16Le));
    }

    #[test]
    fn invalid_utf8_falls_back_to_latin1() {
        let (s, e) = all(b"name\ncaf\xE9\n", Encoding::Auto);
        assert_eq!((s.as_str(), e), ("name\ncafé\n", Encoding::Latin1));
    }

    #[test]
    fn explicit_encoding_wins() {
        let (s, _) = all("café".as_bytes(), Encoding::Latin1);
        assert_eq!(s, "cafÃ©");
    }

    #[test]
    fn multibyte_characters_split_across_reads_survive() {
        struct OneByte<'a>(&'a [u8]);
        impl Read for OneByte<'_> {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                let Some((&b, rest)) = self.0.split_first() else { return Ok(0) };
                buf[0] = b;
                self.0 = rest;
                Ok(1)
            }
        }
        let text = "αβγ,日本語\n";
        let mut t = TextChunks::new(OneByte(text.as_bytes()), Encoding::Utf8);
        let (mut s, mut c) = (String::new(), String::new());
        while t.read_chunk(&mut c).unwrap() {
            s.push_str(&c);
        }
        assert_eq!(s, text);
    }

    #[test]
    fn empty_input_is_empty() {
        assert_eq!(all(b"", Encoding::Auto).0, "");
    }
}

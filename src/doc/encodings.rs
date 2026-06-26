//!
//! Encoding and line-ending detection, transcoding to UTF-8.
//!
//! Reading: auto-detection by BOM, then by the XML declaration, then by a strict
//! trial UTF-8 decode, otherwise cp1251. Saving is always UTF-8 without a BOM (format
//! requirement). Transcoding is streaming — suitable for files up to 1 GB.

use std::io::{Read, Write};
use std::path::Path;

const PROBE_BYTES: usize = 64 * 1024;
const CHUNK: usize = 4 * 1024 * 1024;

/// Outcome of detecting the source file's encoding.
#[derive(Clone, Debug)]
#[allow(dead_code)] // has_bom — informational field
pub struct DetectResult {
    /// Encoding to read with (None == UTF-8).
    pub encoding: Option<&'static encoding_rs::Encoding>,
    pub has_bom: bool,
    /// Label for the status bar ("Windows-1251", …).
    pub label: String,
    /// True if the file is already UTF-8 without a BOM (no transcoding needed).
    pub is_utf8_no_bom: bool,
}

fn utf8_result(label: &str, is_no_bom: bool, has_bom: bool) -> DetectResult {
    DetectResult { encoding: None, has_bom, label: label.to_owned(), is_utf8_no_bom: is_no_bom }
}

/// Detect a file's encoding from its leading bytes: BOM → XML declaration → UTF-8 probe → cp1251.
pub fn detect(path: &Path) -> std::io::Result<DetectResult> {
    let mut f = std::fs::File::open(path)?;
    let mut head = vec![0u8; PROBE_BYTES];
    let n = read_full(&mut f, &mut head)?;
    head.truncate(n);
    Ok(detect_in(&head))
}

fn detect_in(head: &[u8]) -> DetectResult {
    // 1. BOM
    if head.starts_with(b"\xef\xbb\xbf") {
        return utf8_result("UTF-8 BOM", false, true);
    }
    if head.starts_with(b"\xff\xfe") {
        return DetectResult {
            encoding: Some(encoding_rs::UTF_16LE),
            has_bom: true,
            label: "UTF-16 LE".to_owned(),
            is_utf8_no_bom: false,
        };
    }
    if head.starts_with(b"\xfe\xff") {
        return DetectResult {
            encoding: Some(encoding_rs::UTF_16BE),
            has_bom: true,
            label: "UTF-16 BE".to_owned(),
            is_utf8_no_bom: false,
        };
    }

    // 2. XML declaration (ASCII encoding name in the leading bytes)
    if let Some(name) = xml_decl_encoding(&head[..head.len().min(1024)]) {
        let name = name.to_ascii_lowercase();
        match name.as_str() {
            "utf-8" | "utf8" => return utf8_result("UTF-8", true, false),
            "windows-1251" | "cp1251" | "cp-1251" | "windows1251" => {
                return DetectResult {
                    encoding: Some(encoding_rs::WINDOWS_1251),
                    has_bom: false,
                    label: "Windows-1251".to_owned(),
                    is_utf8_no_bom: false,
                };
            }
            "utf-16" | "utf-16le" => {
                return DetectResult {
                    encoding: Some(encoding_rs::UTF_16LE),
                    has_bom: false,
                    label: "UTF-16 LE".to_owned(),
                    is_utf8_no_bom: false,
                };
            }
            "utf-16be" => {
                return DetectResult {
                    encoding: Some(encoding_rs::UTF_16BE),
                    has_bom: false,
                    label: "UTF-16 BE".to_owned(),
                    is_utf8_no_bom: false,
                };
            }
            "koi8-r" => {
                return DetectResult {
                    encoding: Some(encoding_rs::KOI8_R),
                    has_bom: false,
                    label: "KOI8-R".to_owned(),
                    is_utf8_no_bom: false,
                };
            }
            "iso-8859-5" => {
                return DetectResult {
                    encoding: Some(encoding_rs::ISO_8859_5),
                    has_bom: false,
                    label: "ISO-8859-5".to_owned(),
                    is_utf8_no_bom: false,
                };
            }
            _ => {} // unknown name — fall through to the heuristics
        }
    }

    // 3. strict trial UTF-8 (a tail cut off mid-character is forgiven)
    if looks_like_utf8(head) {
        return utf8_result("UTF-8", true, false);
    }

    // 4. fallback — cp1251 (typical for Russian Windows)
    DetectResult {
        encoding: Some(encoding_rs::WINDOWS_1251),
        has_bom: false,
        label: "Windows-1251".to_owned(),
        is_utf8_no_bom: false,
    }
}

/// Extract encoding="..." from the XML declaration without a regex.
fn xml_decl_encoding(head: &[u8]) -> Option<String> {
    let decl_start = head.windows(5).position(|w| w == b"<?xml")?;
    let after = &head[decl_start..];
    let decl_end = after.windows(2).position(|w| w == b"?>").unwrap_or(after.len());
    let decl = &after[..decl_end];
    let key = decl.windows(8).position(|w| w.eq_ignore_ascii_case(b"encoding"))?;
    let mut i = key + 8;
    while i < decl.len() && decl[i].is_ascii_whitespace() {
        i += 1;
    }
    if i >= decl.len() || decl[i] != b'=' {
        return None;
    }
    i += 1;
    while i < decl.len() && decl[i].is_ascii_whitespace() {
        i += 1;
    }
    let quote = *decl.get(i)?;
    if quote != b'"' && quote != b'\'' {
        return None;
    }
    i += 1;
    let end = decl[i..].iter().position(|&b| b == quote)? + i;
    Some(String::from_utf8_lossy(&decl[i..end]).into_owned())
}

/// Strict trial UTF-8 decode; a tail cut off mid-character is forgiven.
fn looks_like_utf8(data: &[u8]) -> bool {
    match std::str::from_utf8(data) {
        Ok(_) => true,
        Err(e) => {
            // error right at the end of the buffer → likely a truncated multi-byte character
            e.error_len().is_none() && e.valid_up_to() >= data.len().saturating_sub(4)
        }
    }
}

fn read_full(f: &mut std::fs::File, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0usize;
    loop {
        let r = f.read(&mut buf[n..])?;
        if r == 0 {
            return Ok(n);
        }
        n += r;
        if n == buf.len() {
            return Ok(n);
        }
    }
}

/// Transcode a file to UTF-8 without a BOM, streaming in 4 MB chunks.
/// Line endings are left untouched. `progress(percent)` is an optional 0..100 callback.
pub fn transcode_to_utf8(
    src_path: &Path,
    dst_path: &Path,
    encoding: &'static encoding_rs::Encoding,
    mut progress: Option<&mut dyn FnMut(u8)>,
) -> std::io::Result<()> {
    let total = std::fs::metadata(src_path)?.len().max(1);
    let mut src = std::fs::File::open(src_path)?;
    let mut dst = std::io::BufWriter::new(std::fs::File::create(dst_path)?);
    // new_decoder(): a BOM matching the encoding is consumed by the decoder
    let mut decoder = encoding.new_decoder();
    let mut inbuf = vec![0u8; CHUNK];
    let mut outbuf = String::with_capacity(CHUNK * 2);
    let mut done = 0u64;
    let mut first = true;
    loop {
        let n = src.read(&mut inbuf)?;
        let last = n == 0;
        let mut input = &inbuf[..n];
        loop {
            outbuf.clear();
            let (result, read, _had_errors) =
                decoder.decode_to_string(input, &mut outbuf, last);
            let mut text = outbuf.as_str();
            // safety net: strip a leading U+FEFF if the decoder did not eat it
            if first && !text.is_empty() {
                text = text.trim_start_matches('\u{feff}');
                first = false;
            }
            dst.write_all(text.as_bytes())?;
            input = &input[read..];
            match result {
                encoding_rs::CoderResult::InputEmpty => break,
                encoding_rs::CoderResult::OutputFull => continue,
            }
        }
        done += n as u64;
        if let Some(cb) = progress.as_deref_mut() {
            cb(((done * 100) / total).min(100) as u8);
        }
        if last {
            break;
        }
    }
    dst.flush()?;
    Ok(())
}

// --- line-ending detection --------------------------------------------------------------

/// The document's line-ending type.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Eol {
    Crlf,
    Lf,
    Cr,
    Mixed,
}

impl Eol {
    pub fn label(self) -> &'static str {
        match self {
            Eol::Crlf => "CRLF",
            Eol::Lf => "LF",
            Eol::Cr => "CR",
            Eol::Mixed => "MIXED",
        }
    }
    /// The line-ending bytes. For MIXED — LF.
    pub fn bytes(self) -> &'static [u8] {
        match self {
            Eol::Crlf => b"\r\n",
            Eol::Lf | Eol::Mixed => b"\n",
            Eol::Cr => b"\r",
        }
    }
}

/// Detect the EOL type from a byte sample. For an empty sample — LF.
pub fn detect_eol(sample: &[u8]) -> Eol {
    let mut crlf = 0usize;
    let mut lone_cr = 0usize;
    let mut lone_lf = 0usize;
    let mut i = 0usize;
    while i < sample.len() {
        match sample[i] {
            b'\r' => {
                if sample.get(i + 1) == Some(&b'\n') {
                    crlf += 1;
                    i += 2;
                    continue;
                }
                lone_cr += 1;
            }
            b'\n' => lone_lf += 1,
            _ => {}
        }
        i += 1;
    }
    let kinds = [crlf, lone_lf, lone_cr].iter().filter(|&&c| c > 0).count();
    match kinds {
        0 => Eol::Lf,
        1 => {
            if crlf > 0 {
                Eol::Crlf
            } else if lone_lf > 0 {
                Eol::Lf
            } else {
                Eol::Cr
            }
        }
        _ => Eol::Mixed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_bom() {
        assert_eq!(detect_in(b"\xef\xbb\xbf<?xml?>").label, "UTF-8 BOM");
        assert_eq!(detect_in(b"\xff\xfe<\0?\0").label, "UTF-16 LE");
        assert_eq!(detect_in(b"\xfe\xff\0<\0?").label, "UTF-16 BE");
    }

    #[test]
    fn detect_xml_decl() {
        let d = detect_in(br#"<?xml version="1.0" encoding="windows-1251"?><a/>"#);
        assert_eq!(d.label, "Windows-1251");
        assert!(!d.is_utf8_no_bom);
        let d = detect_in(br#"<?xml version="1.0" encoding="UTF-8"?><a/>"#);
        assert!(d.is_utf8_no_bom);
    }

    #[test]
    fn detect_probe() {
        let d = detect_in("привет <a/>".as_bytes());
        assert_eq!(d.label, "UTF-8");
        assert!(d.is_utf8_no_bom);
        // cp1251 bytes — not valid UTF-8
        let d = detect_in(&[0xcf, 0xf0, 0xe8, 0xe2, 0xe5, 0xf2]);
        assert_eq!(d.label, "Windows-1251");
    }

    #[test]
    fn probe_forgives_truncated_tail() {
        let mut data = "привет".as_bytes().to_vec();
        data.pop(); // truncate a multi-byte character
        assert!(looks_like_utf8(&data));
    }

    #[test]
    fn eol_detect() {
        assert_eq!(detect_eol(b"a\r\nb\r\n"), Eol::Crlf);
        assert_eq!(detect_eol(b"a\nb"), Eol::Lf);
        assert_eq!(detect_eol(b"a\rb"), Eol::Cr);
        assert_eq!(detect_eol(b"a\r\nb\n"), Eol::Mixed);
        assert_eq!(detect_eol(b""), Eol::Lf);
    }

    #[test]
    fn transcode_cp1251_roundtrip() {
        let dir = std::env::temp_dir().join("justquery-test-enc");
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("src.xml");
        let dst = dir.join("dst.xml");
        let (bytes, _, _) = encoding_rs::WINDOWS_1251.encode("<а>привет</а>");
        std::fs::write(&src, &bytes).unwrap();
        transcode_to_utf8(&src, &dst, encoding_rs::WINDOWS_1251, None).unwrap();
        let out = std::fs::read_to_string(&dst).unwrap();
        assert_eq!(out, "<а>привет</а>");
        let _ = std::fs::remove_file(src);
        let _ = std::fs::remove_file(dst);
    }
}

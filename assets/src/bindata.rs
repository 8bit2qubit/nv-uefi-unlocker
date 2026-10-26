//! NVIDIA BINDATA generated-source array extraction.
//!
//! Ported from gpu-unlock firmware/extract_boot_assets.py `_array_bytes`:
//! a label's `BINDATA_LABEL_<X>_data[] = { 0x.. };` C array, optionally
//! stored as **raw DEFLATE (no gzip header) + gzip CRC32/ISIZE trailer**.

use std::fmt;

#[derive(Debug)]
pub enum BindataError {
    MissingOrDuplicateLabel(String),
    MissingStorageDecl(String),
    ExceedsLimit,
    MalformedDeclaration,
    NonByteToken,
    CompressedSizeMismatch { stored: usize, header: usize },
    BadStream(&'static str),
    ChecksumOrSizeMismatch,
    LengthMismatch { inflated: usize, header: usize },
}

impl fmt::Display for BindataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingOrDuplicateLabel(l) => write!(f, "missing or duplicate label {l}"),
            Self::MissingStorageDecl(l) => write!(f, "missing storage declaration for {l}"),
            Self::ExceedsLimit => write!(f, "asset exceeds extraction limit"),
            Self::MalformedDeclaration => write!(f, "malformed byte-array declaration"),
            Self::NonByteToken => write!(f, "non-byte token in generated array"),
            Self::CompressedSizeMismatch { stored, header } => {
                write!(f, "compressed storage size mismatch {stored} vs {header}")
            }
            Self::BadStream(why) => write!(f, "truncated/trailing/oversized stream: {why}"),
            Self::ChecksumOrSizeMismatch => write!(f, "compressed checksum or size mismatch"),
            Self::LengthMismatch { inflated, header } => {
                write!(f, "inflated {inflated} bytes, header says {header}")
            }
        }
    }
}

const EXTRACT_LIMIT: usize = 16 * 1024 * 1024;

/// Raw-DEFLATE decompression via the well-tested `miniz_oxide`.
/// Trailer (CRC32/ISIZE) is split off by the caller before this call.
fn inflate_raw_deflate(stored: &[u8], max_out: usize) -> Result<Vec<u8>, BindataError> {
    miniz_oxide::inflate::decompress_to_vec_with_limit(stored, max_out)
        .map_err(|_| BindataError::BadStream("inflate failed"))
}

pub struct ArrayBytes {
    pub data: Vec<u8>,
    pub compressed: bool,
    pub declared_size: usize,
}

/// Extract one BINDATA label from a generated-source file's text.
pub fn array_bytes(text: &str, label: &str) -> Result<ArrayBytes, BindataError> {
    let marker = format!("BINDATA_LABEL_{label}_data[]");
    let occurrences = text.matches(&marker).count();
    if occurrences != 1 {
        return Err(BindataError::MissingOrDuplicateLabel(label.into()));
    }
    let start = text.find(&marker).unwrap();
    let window = &text[start.saturating_sub(400)..start];

    let compressed = last_match(window, "COMPRESSION: ")
        .map(|v| v == "YES")
        .ok_or_else(|| BindataError::MissingStorageDecl(label.into()))?;
    let declared = last_match(window, "DATA SIZE (bytes): ")
        .and_then(|v| v.parse::<usize>().ok())
        .ok_or_else(|| BindataError::MissingStorageDecl(label.into()))?;
    if declared == 0 || declared > EXTRACT_LIMIT {
        return Err(BindataError::ExceedsLimit);
    }

    let decl_start = &text[start..];
    let body = decl_start
        .find('{')
        .and_then(|open| text[start + open..].find('}').map(|close| &text[start + open + 1..start + open + close]))
        .ok_or(BindataError::MalformedDeclaration)?;
    let raw: Vec<u8> = body
        .split(',')
        .map(|tok| tok.trim())
        .filter(|tok| !tok.is_empty() && *tok != "};")
        .map(|tok| {
            let t = tok.trim_end_matches(';').trim();
            u8::from_str_radix(t.trim_start_matches("0x"), 16)
                .map_err(|_| BindataError::NonByteToken)
        })
        .collect::<Result<_, _>>()?;

    let (data, _consumed) = if compressed {
        let header_stored = last_match(window, "COMPRESSED SIZE (bytes): ")
            .and_then(|v| v.parse::<usize>().ok())
            .ok_or_else(|| BindataError::MissingStorageDecl(label.into()))?;
        if raw.len() != header_stored {
            return Err(BindataError::CompressedSizeMismatch { stored: raw.len(), header: header_stored });
        }
        // Raw deflate body is raw.len() - 8 (trailer); trailer = CRC32 + ISIZE.
        if raw.len() < 8 {
            return Err(BindataError::BadStream("no room for trailer"));
        }
        let (deflate_body, trailer) = raw.split_at(raw.len() - 8);
        let out = inflate_raw_deflate(deflate_body, declared + 1)?;
        if out.len() != declared {
            return Err(BindataError::BadStream("size mismatch after inflate"));
        }
        let (crc_bytes, size_bytes) = trailer.split_at(4);
        let crc = u32::from_le_bytes(crc_bytes.try_into().unwrap());
        let size = u32::from_le_bytes(size_bytes.try_into().unwrap());
        if size as usize != declared || crc != crc32fast::hash(&out) {
            return Err(BindataError::ChecksumOrSizeMismatch);
        }
        (out, raw.len())
    } else {
        if raw.len() != declared {
            return Err(BindataError::LengthMismatch { inflated: raw.len(), header: declared });
        }
        let n = raw.len();
        (raw, n)
    };

    if data.len() != declared {
        return Err(BindataError::LengthMismatch { inflated: data.len(), header: declared });
    }
    Ok(ArrayBytes { data, compressed, declared_size: declared })
}

fn last_match<'a>(window: &'a str, prefix: &str) -> Option<&'a str> {
    window.match_indices(prefix).last().map(|(i, _)| {
        window[i + prefix.len()..]
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .trim_end_matches("*/")
            .trim()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_source(data: &[u8], compressed: bool) -> String {
        let (body, compression, dsize) = if compressed {
            // stored (uncompressed) deflate blocks: header byte 0x01 for
            // final block, LEN, NLEN, then literal bytes; plus gzip trailer.
            let n = data.len() as u16;
            let mut z = vec![0x01];
            z.extend_from_slice(&n.to_le_bytes());
            z.extend_from_slice(&(!n).to_le_bytes());
            z.extend_from_slice(data);
            let crc = crc32fast::hash(data);
            z.extend_from_slice(&crc.to_le_bytes());
            z.extend_from_slice(&(data.len() as u32).to_le_bytes());
            (z, "YES", data.len())
        } else {
            (data.to_vec(), "NO", data.len())
        };
        let hex: Vec<String> = body.iter().map(|b| format!("0x{b:02x}")).collect();
        format!(
            "/* header noise */\n\
             /* COMPRESSION: {compression} */\n\
             /* DATA SIZE (bytes): {dsize} */\n\
             /* COMPRESSED SIZE (bytes): {} */\n\
             static const NvU8 BINDATA_LABEL_TEST_data[] = {{\n  {}\n}};\n",
            body.len(),
            hex.join(", "),
        )
    }

    #[test]
    fn uncompressed_roundtrip() {
        let src = build_source(b"hello bindata", false);
        let out = array_bytes(&src, "TEST").unwrap();
        assert!(!out.compressed);
        assert_eq!(out.data, b"hello bindata");
    }

    #[test]
    fn compressed_roundtrip_with_trailer_validation() {
        let src = build_source(b"deflate me please, longer than zero", true);
        let out = array_bytes(&src, "TEST").unwrap();
        assert!(out.compressed);
        assert_eq!(out.data, b"deflate me please, longer than zero");
    }

    #[test]
    fn duplicate_label_rejected() {
        let src = build_source(b"x", false);
        let doubled = format!("{src}{src}");
        assert!(matches!(
            array_bytes(&doubled, "TEST"),
            Err(BindataError::MissingOrDuplicateLabel(_))
        ));
    }

    #[test]
    fn corrupt_trailer_rejected() {
        let mut src = build_source(b"payload data", true);
        // flip one hex char inside the CRC trailer (last 8 bytes region)
        let pos = src.rfind("0x").unwrap();
        src.replace_range(pos + 2..pos + 4, if &src[pos + 2..pos + 4] == "00" { "ff" } else { "00" });
        assert!(array_bytes(&src, "TEST").is_err());
    }
}

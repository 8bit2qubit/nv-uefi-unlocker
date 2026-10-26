//! Single-member extraction from pinned tar archives (.tar.xz / .tar.zst).
//! Ported from generate_v67.py's guarded tarfile usage: exact-name match,
//! duplicate rejection, regular-files only, 128 MiB member cap.

use std::fs::File;
use std::io::Read;
use std::path::Path;

const MEMBER_LIMIT: u64 = 128 * 1024 * 1024;

#[derive(Debug)]
pub enum ArchiveError {
    Io(std::io::Error),
    DuplicateMember(String),
    NotARegularFile(String),
    MemberTooLarge(String),
    MissingMember(String),
}

impl From<std::io::Error> for ArchiveError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl std::fmt::Display for ArchiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io error: {e}"),
            Self::DuplicateMember(n) => write!(f, "duplicate package member: {n}"),
            Self::NotARegularFile(n) => write!(f, "not a regular file in package: {n}"),
            Self::MemberTooLarge(n) => write!(f, "member exceeds limit: {n}"),
            Self::MissingMember(n) => write!(f, "missing package member: {n}"),
        }
    }
}

fn read_member<R: Read>(archive: &mut tar::Archive<R>, name: &str) -> Result<Vec<u8>, ArchiveError> {
    let mut result: Option<Vec<u8>> = None;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_string_lossy().replace('\\', "/");
        if path != name {
            continue;
        }
        if result.is_some() {
            return Err(ArchiveError::DuplicateMember(name.into()));
        }
        let header = entry.header();
        if !header.entry_type().is_file() || header.size()? > MEMBER_LIMIT {
            return Err(ArchiveError::NotARegularFile(name.into()));
        }
        let mut data = Vec::with_capacity(header.size()? as usize);
        entry.read_to_end(&mut data)?;
        result = Some(data);
    }
    result.ok_or_else(|| ArchiveError::MissingMember(name.into()))
}

pub fn extract_member_xz(path: &Path, name: &str) -> Result<Vec<u8>, ArchiveError> {
    let file = File::open(path)?;
    let decoder = xz2::read::XzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    read_member(&mut archive, name)
}

pub fn extract_member_zst(path: &Path, name: &str) -> Result<Vec<u8>, ArchiveError> {
    let file = File::open(path)?;
    let decoder = zstd::stream::read::Decoder::new(file)?;
    let mut archive = tar::Archive::new(decoder);
    read_member(&mut archive, name)
}

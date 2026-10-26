//! Minimal read-only ELF64 little-endian section walker for GSP firmware
//! images (.fwimage / .fwsignature_* sections; measured 2026-10-25:
//! gsp_tu10x.bin carries .fwsignature_tu10x/_tu11x/_ga100, gsp_ga10x.bin
//! carries .fwsignature_ga10x/_ad10x plus GB/GR families).

#[derive(Debug)]
pub enum ElfError {
    NotAnElf,
    Truncated,
    SectionNotFound(String),
}

impl std::fmt::Display for ElfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAnElf => write!(f, "not an ELF64 LE image"),
            Self::Truncated => write!(f, "image truncated"),
            Self::SectionNotFound(n) => write!(f, "section not found: {n}"),
        }
    }
}

pub struct Section {
    pub name: String,
    pub offset: u64,
    pub size: u64,
}

fn u16le(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(o..o + 2)?.try_into().ok()?))
}
fn u32le(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?))
}
fn u64le(d: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_le_bytes(d.get(o..o + 8)?.try_into().ok()?))
}

/// All named sections in the image.
pub fn sections(image: &[u8]) -> Result<Vec<Section>, ElfError> {
    if image.len() < 64 || &image[0..4] != b"\x7fELF" {
        return Err(ElfError::NotAnElf);
    }
    if image[4] != 2 || image[5] != 1 {
        return Err(ElfError::NotAnElf); // ELFCLASS64, little-endian only
    }
    let e_shoff = u64le(image, 0x28).ok_or(ElfError::Truncated)?;
    let e_shentsize = u16le(image, 0x3a).ok_or(ElfError::Truncated)? as usize;
    let e_shnum = u16le(image, 0x3c).ok_or(ElfError::Truncated)? as usize;
    let e_shstrndx = u16le(image, 0x3e).ok_or(ElfError::Truncated)? as usize;
    if e_shentsize < 64 {
        return Err(ElfError::Truncated);
    }
    let sh = |i: usize| -> Option<&[u8]> { image.get(e_shoff as usize + i * e_shentsize..)?.get(..e_shentsize) };
    let strtab_hdr = sh(e_shstrndx).ok_or(ElfError::Truncated)?;
    let str_off = u64le(strtab_hdr, 0x18).ok_or(ElfError::Truncated)? as usize;
    let str_size = u64le(strtab_hdr, 0x20).ok_or(ElfError::Truncated)? as usize;
    let strtab = image.get(str_off..str_off + str_size).ok_or(ElfError::Truncated)?;

    let mut out = Vec::new();
    for i in 0..e_shnum {
        let h = sh(i).ok_or(ElfError::Truncated)?;
        let name_off = u32le(h, 0).ok_or(ElfError::Truncated)? as usize;
        let typ = u32le(h, 4).unwrap_or(0);
        if typ == 0 {
            continue; // SHT_NULL
        }
        let end = strtab[name_off..].iter().position(|&b| b == 0).map(|p| name_off + p).unwrap_or(strtab.len());
        let name = String::from_utf8_lossy(&strtab[name_off..end]).into_owned();
        out.push(Section {
            name,
            offset: u64le(h, 0x18).ok_or(ElfError::Truncated)?,
            size: u64le(h, 0x20).ok_or(ElfError::Truncated)?,
        });
    }
    Ok(out)
}

/// Bytes of one named section.
pub fn section_bytes<'a>(image: &'a [u8], name: &str) -> Result<&'a [u8], ElfError> {
    let s = sections(image)?
        .into_iter()
        .find(|s| s.name == name)
        .ok_or_else(|| ElfError::SectionNotFound(name.into()))?;
    image
        .get(s.offset as usize..(s.offset + s.size) as usize)
        .ok_or(ElfError::Truncated)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand-built minimal ELF64 LE with two sections + shstrtab.
    fn fixture() -> Vec<u8> {
        let mut img = vec![0u8; 4096];
        img[0..4].copy_from_slice(b"\x7fELF");
        img[4] = 2; // 64-bit
        img[5] = 1; // LE
        img[6] = 1; // version

        let shstrtab: &[u8] = b"\0.fwimage\0.fwsignature\0.shstrtab\0";
        let off_fwimage = 0x800;
        let off_fwsig = 0x900;
        let off_str = 0xA00;
        img[off_fwimage..off_fwimage + 4].copy_from_slice(b"CODE");
        img[off_fwsig..off_fwsig + 4].copy_from_slice(b"SIG!");

        let es = 64usize;
        let e_shoff = 0xC00;
        let shnum = 4usize; // NULL + fwimage + fwsignature + shstrtab
        // write section data
        let mut put_sh = |i: usize, name_off: u32, typ: u32, off: u64, size: u64| {
            let h = e_shoff + i * es;
            img[h..h + 4].copy_from_slice(&name_off.to_le_bytes());
            img[h + 4..h + 8].copy_from_slice(&typ.to_le_bytes());
            img[h + 0x18..h + 0x20].copy_from_slice(&off.to_le_bytes());
            img[h + 0x20..h + 0x28].copy_from_slice(&size.to_le_bytes());
        };
        put_sh(0, 0, 0, 0, 0);
        put_sh(1, 1, 1, off_fwimage as u64, 4);
        put_sh(2, 10, 1, off_fwsig as u64, 4);
        put_sh(3, 23, 3, off_str as u64, shstrtab.len() as u64);
        img[off_str..off_str + shstrtab.len()].copy_from_slice(shstrtab);

        img[0x28..0x30].copy_from_slice(&(e_shoff as u64).to_le_bytes());
        img[0x3a..0x3c].copy_from_slice(&(es as u16).to_le_bytes());
        img[0x3c..0x3e].copy_from_slice(&(shnum as u16).to_le_bytes());
        img[0x3e..0x40].copy_from_slice(&3u16.to_le_bytes());
        img
    }

    #[test]
    fn fixture_sections_and_bytes() {
        let img = fixture();
        let names: Vec<String> = sections(&img).unwrap().into_iter().map(|s| s.name).collect();
        assert!(names.contains(&".fwimage".to_string()));
        assert!(names.contains(&".fwsignature".to_string()));
        assert_eq!(section_bytes(&img, ".fwimage").unwrap(), b"CODE");
        assert_eq!(section_bytes(&img, ".fwsignature").unwrap(), b"SIG!");
        assert!(section_bytes(&img, ".missing").is_err());
    }

    /// Real-image anchor (skipped when the pinned input is absent).
    #[test]
    fn gsp_tu10x_when_present() {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../assets/out/gsp_tu10x.bin");
        let Ok(img) = std::fs::read(&p) else { return };
        let secs = sections(&img).expect("tu10x GSP must parse as ELF64");
        let names: Vec<&str> = secs.iter().map(|s| s.name.as_str()).collect();
        for expect in [".fwimage", ".fwsignature_tu10x", ".fwversion"] {
            assert!(names.contains(&expect), "missing {expect} in {names:?}");
        }
        let sig = section_bytes(&img, ".fwsignature_tu10x").unwrap();
        assert_eq!(sig.len(), 0x1000, "RSA3K signature area size");
    }
}

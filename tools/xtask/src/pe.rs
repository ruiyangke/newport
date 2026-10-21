//! Windows PE header checks for staged executables and libraries.
use crate::util::Arch;
use anyhow::{bail, ensure, Result};

fn machine(arch: Arch) -> u16 {
    match arch {
        Arch::X86_64 => 0x8664,
        Arch::Aarch64 => 0xaa64,
    }
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

/// Check a PE32+ image for the architecture, and whether it is a DLL or an executable.
pub fn verify(bytes: &[u8], arch: Arch, dll: bool) -> Result<()> {
    ensure!(
        bytes.len() >= 64 && bytes.starts_with(b"MZ"),
        "Invalid PE executable"
    );
    let offset = u32_at(bytes, 0x3c) as usize;
    if offset < 64 || offset.saturating_add(26) > bytes.len() {
        bail!("PE architecture or executable type does not match the package");
    }
    let characteristics = u16_at(bytes, offset + 22);
    ensure!(
        u32_at(bytes, offset) == 0x4550
            && u16_at(bytes, offset + 4) == machine(arch)
            && u16_at(bytes, offset + 24) == 0x20b
            && characteristics & 2 != 0
            && (characteristics & 0x2000 != 0) == dll,
        "PE architecture or executable type does not match the package"
    );
    Ok(())
}

/// A minimal PE32+ header for tests.
#[cfg(test)]
pub fn fixture(arch: Arch, dll: bool) -> Vec<u8> {
    let mut bytes = vec![0; 256];
    bytes[..2].copy_from_slice(b"MZ");
    bytes[0x3c..0x40].copy_from_slice(&64u32.to_le_bytes());
    bytes[64..68].copy_from_slice(&0x4550u32.to_le_bytes());
    bytes[68..70].copy_from_slice(&machine(arch).to_le_bytes());
    bytes[86..88].copy_from_slice(&(2u16 | if dll { 0x2000 } else { 0 }).to_le_bytes());
    bytes[88..90].copy_from_slice(&0x20bu16.to_le_bytes());
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_matching_images() {
        verify(&fixture(Arch::X86_64, false), Arch::X86_64, false).unwrap();
        verify(&fixture(Arch::Aarch64, true), Arch::Aarch64, true).unwrap();
    }

    #[test]
    fn rejects_truncated_offsets_and_executable_library_confusion() {
        let mut bytes = fixture(Arch::X86_64, false);
        let error = verify(&bytes, Arch::X86_64, true).unwrap_err();
        assert!(error.to_string().contains("type"), "{error}");
        let error = verify(&bytes, Arch::Aarch64, false).unwrap_err();
        assert!(error.to_string().contains("architecture"), "{error}");
        bytes[0x3c..0x40].copy_from_slice(&u32::MAX.to_le_bytes());
        let error = verify(&bytes, Arch::X86_64, false).unwrap_err();
        assert!(error.to_string().contains("architecture"), "{error}");
        let error = verify(&[], Arch::X86_64, false).unwrap_err();
        assert!(error.to_string().contains("Invalid PE"), "{error}");
    }
}

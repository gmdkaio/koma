use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

pub fn write_stored_zip(dest: &Path, files: &[(String, &Path)]) -> Result<(), String> {
    let mut out =
        File::create(dest).map_err(|e| format!("failed to create {}: {e}", dest.display()))?;
    let mut central = Vec::new();
    let mut offset: u32 = 0;

    for (name, path) in files {
        let data =
            fs::read(path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
        let name_bytes = name.as_bytes();
        let crc = crc32(&data);
        let len = u32::try_from(data.len())
            .map_err(|_| format!("file too large for zip: {}", path.display()))?;
        let name_len = u16::try_from(name_bytes.len())
            .map_err(|_| format!("filename too long for zip: {name}"))?;

        let mut local = Vec::with_capacity(30 + name_bytes.len());
        local.extend_from_slice(&0x04034b50u32.to_le_bytes());
        local.extend_from_slice(&20u16.to_le_bytes()); // version needed
        local.extend_from_slice(&0x0800u16.to_le_bytes()); // UTF-8 flag
        local.extend_from_slice(&0u16.to_le_bytes()); // method: STORED
        local.extend_from_slice(&0u16.to_le_bytes()); // mod time
        local.extend_from_slice(&0u16.to_le_bytes()); // mod date
        local.extend_from_slice(&crc.to_le_bytes());
        local.extend_from_slice(&len.to_le_bytes());
        local.extend_from_slice(&len.to_le_bytes());
        local.extend_from_slice(&name_len.to_le_bytes());
        local.extend_from_slice(&0u16.to_le_bytes()); // extra len
        local.extend_from_slice(name_bytes);

        out.write_all(&local)
            .and_then(|_| out.write_all(&data))
            .map_err(|e| format!("failed writing zip entry {name}: {e}"))?;

        central.extend_from_slice(&0x02014b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes()); // version made by
        central.extend_from_slice(&20u16.to_le_bytes()); // version needed
        central.extend_from_slice(&0x0800u16.to_le_bytes()); // UTF-8 flag
        central.extend_from_slice(&0u16.to_le_bytes()); // STORED
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&len.to_le_bytes());
        central.extend_from_slice(&len.to_le_bytes());
        central.extend_from_slice(&name_len.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes()); // extra
        central.extend_from_slice(&0u16.to_le_bytes()); // comment
        central.extend_from_slice(&0u16.to_le_bytes()); // disk
        central.extend_from_slice(&0u16.to_le_bytes()); // internal attr
        central.extend_from_slice(&0u32.to_le_bytes()); // external attr
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name_bytes);

        offset = offset
            .checked_add(local.len() as u32)
            .and_then(|v| v.checked_add(len))
            .ok_or_else(|| "zip archive exceeds 4 GiB limit".to_string())?;
    }

    let central_len = u32::try_from(central.len())
        .map_err(|_| "zip central directory too large".to_string())?;
    let count = u16::try_from(files.len()).map_err(|_| "too many files for zip".to_string())?;

    out.write_all(&central)
        .map_err(|e| format!("failed writing zip central dir: {e}"))?;

    let mut eocd = Vec::with_capacity(22);
    eocd.extend_from_slice(&0x06054b50u32.to_le_bytes());
    eocd.extend_from_slice(&0u16.to_le_bytes());
    eocd.extend_from_slice(&0u16.to_le_bytes());
    eocd.extend_from_slice(&count.to_le_bytes());
    eocd.extend_from_slice(&count.to_le_bytes());
    eocd.extend_from_slice(&central_len.to_le_bytes());
    eocd.extend_from_slice(&offset.to_le_bytes());
    eocd.extend_from_slice(&0u16.to_le_bytes());

    out.write_all(&eocd)
        .map_err(|e| format!("failed writing zip eocd: {e}"))?;
    Ok(())
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &b in bytes {
        crc ^= u32::from(b);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320u32 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn writes_valid_stored_zip() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("koma-zip-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        let f1 = dir.join("a.txt");
        fs::write(&f1, b"hello koma").unwrap();
        let archive = dir.join("out.zip");

        write_stored_zip(&archive, &[("a.txt".to_string(), f1.as_path())]).unwrap();
        let bytes = fs::read(&archive).unwrap();
        assert_eq!(&bytes[0..4], b"PK\x03\x04");
        assert!(bytes.windows(10).any(|w| w == b"hello koma"));
        assert_eq!(&bytes[bytes.len() - 22..bytes.len() - 18], b"PK\x05\x06");

        let _ = fs::remove_dir_all(&dir);
    }
}

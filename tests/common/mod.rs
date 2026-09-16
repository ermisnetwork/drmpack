/// Recursively find an MP4 box by type in binary data.
pub fn find_box<'a>(data: &'a [u8], box_type: &[u8; 4]) -> Option<&'a [u8]> {
    let mut offset = 0usize;
    while offset.checked_add(8)? <= data.len() {
        let size32 = u32::from_be_bytes(data[offset..offset + 4].try_into().ok()?);
        let (header_size, box_size) = match size32 {
            0 => (8usize, data.len().checked_sub(offset)?),
            1 => {
                if offset.checked_add(16)? > data.len() {
                    return None;
                }
                (
                    16usize,
                    usize::try_from(u64::from_be_bytes(
                        data[offset + 8..offset + 16].try_into().ok()?,
                    ))
                    .ok()?,
                )
            }
            size => (8usize, size as usize),
        };
        if box_size < header_size || offset.checked_add(box_size)? > data.len() {
            return None;
        }

        let current_type: &[u8; 4] = data[offset + 4..offset + 8].try_into().ok()?;
        let current_box = &data[offset..offset + box_size];
        if current_type == box_type {
            return Some(current_box);
        }
        if matches!(
            current_type,
            b"moov"
                | b"trak"
                | b"mdia"
                | b"minf"
                | b"stbl"
                | b"stsd"
                | b"encv"
                | b"enca"
                | b"sinf"
                | b"schi"
                | b"moof"
                | b"traf"
        ) {
            let child_offset = if current_type == b"stsd" {
                header_size + 8
            } else if current_type == b"encv" {
                header_size + 78
            } else if current_type == b"enca" {
                header_size + 28
            } else {
                header_size
            };
            if child_offset <= current_box.len() {
                if let Some(found) = find_box(&current_box[child_offset..], box_type) {
                    return Some(found);
                }
            }
        }
        offset += box_size;
    }
    None
}

/// Verify if `data` is a valid self-initializing Single-File media container (e.g. DASH onDemand / HLS byte-range)
/// containing `ftyp`, `moov`, and `sidx` (Segment Index) boxes.
#[allow(dead_code)]
pub fn is_complete_single_file_mp4(data: &[u8]) -> bool {
    find_box(data, b"ftyp").is_some()
        && find_box(data, b"moov").is_some()
        && find_box(data, b"sidx").is_some()
}

/// RAII guard ensuring temporary test directories in `/tmp` are cleaned up even on assertion failure or panic.
#[allow(dead_code)]
pub struct TempDirGuard {
    path: std::path::PathBuf,
}

#[allow(dead_code)]
impl TempDirGuard {
    pub fn new(prefix: &str) -> Self {
        let path = std::env::temp_dir().join(format!("{}_{}", prefix, uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).expect("failed to create temp test dir");
        Self { path }
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl std::ops::Deref for TempDirGuard {
    type Target = std::path::Path;

    fn deref(&self) -> &Self::Target {
        &self.path
    }
}

impl AsRef<std::path::Path> for TempDirGuard {
    fn as_ref(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        if self.path.exists() {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

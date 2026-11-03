use std::collections::HashSet;
use std::fs::Metadata;
use std::io;
use std::path::Path;
use std::time::SystemTime;
use walkdir::WalkDir;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Space {
    pub total: u64,
    pub available: u64,
}

// statvfs field widths differ per unix target, so the casts are needed.
#[cfg(unix)]
#[allow(clippy::unnecessary_cast)]
pub fn space(path: &Path) -> io::Result<Space> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let c_path = CString::new(path.as_os_str().as_bytes())
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: c_path is a valid NUL-terminated string and stat points to writable statvfs storage.
    if unsafe { libc::statvfs(c_path.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: statvfs returned 0, so it fully initialised stat.
    let stat = unsafe { stat.assume_init() };
    let block = stat.f_frsize as u64;
    Ok(Space {
        total: stat.f_blocks as u64 * block,
        available: stat.f_bavail as u64 * block,
    })
}

#[cfg(windows)]
pub fn space(path: &Path) -> io::Result<Space> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let (mut available, mut total, mut free) = (0u64, 0u64, 0u64);
    // SAFETY: wide is NUL-terminated and the three out pointers reference live u64s.
    if unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut available, &mut total, &mut free) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Space { total, available })
}

#[cfg(unix)]
fn usage(meta: &Metadata, seen: &mut HashSet<(u64, u64)>) -> u64 {
    use std::os::unix::fs::MetadataExt;

    let hard_linked_again =
        !meta.is_dir() && meta.nlink() > 1 && !seen.insert((meta.dev(), meta.ino()));
    if hard_linked_again {
        return 0;
    }
    meta.blocks() * 512
}

#[cfg(windows)]
fn usage(meta: &Metadata, _seen: &mut HashSet<(u64, u64)>) -> u64 {
    meta.len()
}

pub fn dir_size(path: &Path) -> u64 {
    let mut seen = HashSet::new();
    WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter_map(|entry| entry.metadata().ok())
        .map(|meta| usage(&meta, &mut seen))
        .sum()
}

/// Newest mtime of the folder and its direct children; links are not followed.
pub fn last_use(path: &Path) -> io::Result<SystemTime> {
    let mut newest = std::fs::symlink_metadata(path)?.modified()?;
    for entry in std::fs::read_dir(path)?.flatten() {
        let Ok(modified) = std::fs::symlink_metadata(entry.path()).and_then(|m| m.modified())
        else {
            continue;
        };
        newest = newest.max(modified);
    }
    Ok(newest)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VolumeId(String);

#[cfg(unix)]
pub fn volume_id(path: &Path) -> io::Result<VolumeId> {
    use std::os::unix::fs::MetadataExt;

    Ok(VolumeId(std::fs::metadata(path)?.dev().to_string()))
}

#[cfg(windows)]
pub fn volume_id(path: &Path) -> io::Result<VolumeId> {
    use std::path::Component;

    let canonical = dunce::canonicalize(path)?;
    match canonical.components().next() {
        Some(Component::Prefix(prefix)) => Ok(VolumeId(
            prefix.as_os_str().to_string_lossy().to_uppercase(),
        )),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path has no drive or share prefix",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn space_reports_total_and_available() {
        let tmp = TempDir::new().unwrap();
        let space = space(tmp.path()).unwrap();
        assert!(space.available > 0);
        assert!(space.total >= space.available);
    }

    #[test]
    fn dir_size_counts_nested_and_hidden_files() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("a/.hidden")).unwrap();
        fs::write(tmp.path().join("a/.hidden/f"), vec![1u8; 20_000]).unwrap();
        fs::write(tmp.path().join("g"), vec![1u8; 10_000]).unwrap();
        assert!(dir_size(tmp.path()) >= 30_000);
    }

    #[cfg(unix)]
    #[test]
    fn dir_size_does_not_follow_symlinks() {
        let tmp = TempDir::new().unwrap();
        let big = TempDir::new().unwrap();
        fs::write(big.path().join("big"), vec![1u8; 1_000_000]).unwrap();
        std::os::unix::fs::symlink(big.path().join("big"), tmp.path().join("link")).unwrap();
        std::os::unix::fs::symlink(big.path(), tmp.path().join("dirlink")).unwrap();
        assert!(dir_size(tmp.path()) < 100_000);
    }

    #[cfg(unix)]
    #[test]
    fn dir_size_counts_hard_links_once() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("a"), vec![1u8; 1_000_000]).unwrap();
        fs::hard_link(tmp.path().join("a"), tmp.path().join("b")).unwrap();
        assert!(dir_size(tmp.path()) < 1_500_000);
    }

    fn set_mtime(path: &Path, time: SystemTime) {
        let file = fs::File::options().write(true).open(path).unwrap();
        file.set_modified(time).unwrap();
    }

    #[test]
    fn last_use_is_newest_of_folder_and_children() {
        let tmp = TempDir::new().unwrap();
        let old = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000);
        let newer = old + std::time::Duration::from_secs(500);
        fs::write(tmp.path().join("a"), "").unwrap();
        fs::write(tmp.path().join("b"), "").unwrap();
        set_mtime(&tmp.path().join("a"), old);
        set_mtime(&tmp.path().join("b"), newer);
        let folder_time = fs::metadata(tmp.path()).unwrap().modified().unwrap();
        assert_eq!(last_use(tmp.path()).unwrap(), folder_time.max(newer));
    }

    #[test]
    fn last_use_ignores_grandchildren() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join("sub")).unwrap();
        fs::write(tmp.path().join("sub/deep"), "").unwrap();
        let future = SystemTime::now() + std::time::Duration::from_secs(10_000);
        set_mtime(&tmp.path().join("sub/deep"), future);
        assert!(last_use(tmp.path()).unwrap() < future);
    }

    #[cfg(unix)]
    #[test]
    fn last_use_does_not_follow_links() {
        let tmp = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let future = SystemTime::now() + std::time::Duration::from_secs(10_000);
        fs::write(outside.path().join("f"), "").unwrap();
        set_mtime(&outside.path().join("f"), future);
        std::os::unix::fs::symlink(outside.path().join("f"), tmp.path().join("link")).unwrap();
        assert!(last_use(tmp.path()).unwrap() < future);
    }

    #[test]
    fn last_use_of_missing_folder_is_error() {
        let tmp = TempDir::new().unwrap();
        assert!(last_use(&tmp.path().join("missing")).is_err());
    }

    #[test]
    fn volume_id_is_equal_within_a_directory() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join("sub")).unwrap();
        assert_eq!(
            volume_id(tmp.path()).unwrap(),
            volume_id(&tmp.path().join("sub")).unwrap()
        );
    }
}

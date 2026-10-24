//! Wallpaper Engine `PKGV` archive.
//!
//! ```text
//! u32 length | magic bytes          "PKGV0001" .. "PKGV0022", no trailing NUL
//! u32 count
//! repeated count times:
//!     u32 length | utf-8 path
//!     u32 offset                  relative to the first byte after the directory
//!     u32 length
//! file bytes
//! ```
//!
//! Offsets are not absolute. [`Package::data_offset`] is the base added to each
//! entry offset. Paths are stored with `\` rewritten to `/`. A path that is
//! absolute or contains `.` or `..` is rejected so a later extractor cannot
//! write outside its destination.

use std::path::Path;

use crate::cursor::Cursor;
use crate::error::Error;
use crate::limits;

/// One file inside a [`Package`].
pub struct Entry {
    /// Archive-relative path using `/` separators.
    pub path: String,
    /// Offset from [`Package::data_offset`], as stored in the directory.
    pub offset: u32,
    /// Byte length stored in the directory. Matches `bytes.len()`.
    pub length: u32,
    /// File contents. Owned so the caller can drop the archive buffer.
    pub bytes: Vec<u8>,
}

impl std::fmt::Debug for Entry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Entry")
            .field("path", &self.path)
            .field("offset", &self.offset)
            .field("length", &self.length)
            .field("bytes", &self.bytes.len())
            .finish()
    }
}

/// A parsed package. Entry order matches the directory.
pub struct Package {
    /// Magic string, including the `PKGV` prefix and the version digits.
    pub magic: String,
    /// Absolute offset of the first payload byte.
    pub data_offset: u64,
    entries: Vec<Entry>,
}

impl std::fmt::Debug for Package {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Package")
            .field("magic", &self.magic)
            .field("data_offset", &self.data_offset)
            .field("entries", &self.entries)
            .finish()
    }
}

impl Package {
    /// Read an archive from disk and parse it.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        let bytes = std::fs::read(path).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse(&bytes)
    }

    /// Parse an archive that is already in memory.
    pub fn parse(data: &[u8]) -> Result<Self, Error> {
        let mut cursor = Cursor::new(data);
        let magic = cursor.sized_string("pkg magic", limits::MAX_STRING_LEN)?;
        if !magic.starts_with("PKGV") {
            return Err(Error::BadMagic {
                context: "pkg magic",
                found: magic,
            });
        }
        let count = cursor.u32("pkg entry count")?;
        if count > limits::MAX_ENTRY_COUNT {
            return Err(Error::Limit {
                context: "pkg entry count",
                value: u64::from(count),
                limit: u64::from(limits::MAX_ENTRY_COUNT),
            });
        }
        let mut pending = Vec::with_capacity(usize::try_from(count).unwrap_or(0));
        for _ in 0..count {
            let raw_path = cursor.sized_string("pkg entry path", limits::MAX_STRING_LEN)?;
            let path = normalize_path(&raw_path)?;
            let offset = cursor.u32("pkg entry offset")?;
            let length = cursor.u32("pkg entry length")?;
            pending.push(Pending {
                path,
                offset,
                length,
            });
        }
        let data_offset =
            u64::try_from(cursor.position()).map_err(|_| Error::Invariant("data offset"))?;
        let mut entries = Vec::with_capacity(pending.len());
        for entry in pending {
            let start = data_offset
                .checked_add(u64::from(entry.offset))
                .ok_or(Error::Invariant("entry start"))?;
            let end = start
                .checked_add(u64::from(entry.length))
                .ok_or(Error::Invariant("entry end"))?;
            let file_len = u64::try_from(data.len()).map_err(|_| Error::Invariant("file len"))?;
            if end > file_len {
                return Err(Error::BadRange {
                    path: entry.path,
                    start,
                    length: u64::from(entry.length),
                    file_len,
                });
            }
            let start_usize =
                usize::try_from(start).map_err(|_| Error::Invariant("entry start usize"))?;
            let end_usize =
                usize::try_from(end).map_err(|_| Error::Invariant("entry end usize"))?;
            let bytes = data
                .get(start_usize..end_usize)
                .ok_or(Error::Invariant("entry slice"))?
                .to_vec();
            entries.push(Entry {
                path: entry.path,
                offset: entry.offset,
                length: entry.length,
                bytes,
            });
        }
        Ok(Self {
            magic,
            data_offset,
            entries,
        })
    }

    /// Directory entries, in archive order.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the directory is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// First entry with this path.
    ///
    /// Lookup rewrites `\` to `/`. An unsafe path returns `None`.
    pub fn get(&self, path: &str) -> Option<&[u8]> {
        let key = normalize_path(path).ok()?;
        self.entries
            .iter()
            .find(|entry| entry.path == key)
            .map(|entry| entry.bytes.as_slice())
    }
}

struct Pending {
    path: String,
    offset: u32,
    length: u32,
}

fn normalize_path(raw: &str) -> Result<String, Error> {
    if raw.is_empty() || raw.contains('\0') {
        return Err(Error::UnsafePath(raw.to_owned()));
    }
    let normalized = raw.replace('\\', "/");
    if normalized.starts_with('/') || has_drive_prefix(&normalized) {
        return Err(Error::UnsafePath(normalized));
    }
    let mut parts = Vec::new();
    for part in normalized.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Err(Error::UnsafePath(normalized));
        }
        parts.push(part);
    }
    Ok(parts.join("/"))
}

fn has_drive_prefix(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{must, push_sized, push_u32};

    fn sample_package() -> Vec<u8> {
        let mut buf = Vec::new();
        push_sized(&mut buf, "PKGV0001");
        push_u32(&mut buf, 2);
        push_sized(&mut buf, "a.txt");
        push_u32(&mut buf, 0);
        push_u32(&mut buf, 5);
        push_sized(&mut buf, r"dir\b.txt");
        push_u32(&mut buf, 5);
        push_u32(&mut buf, 5);
        buf.extend_from_slice(b"HelloWorld");
        buf
    }

    #[test]
    fn offsets_are_relative_to_the_byte_after_the_directory() {
        let package = must(Package::parse(&sample_package()));
        assert_eq!(package.magic, "PKGV0001");
        assert_eq!(package.len(), 2);
        assert_eq!(package.get("a.txt"), Some(b"Hello".as_slice()));
        assert_eq!(package.get(r"dir\b.txt"), Some(b"World".as_slice()));
        assert_eq!(package.get("dir/b.txt"), Some(b"World".as_slice()));
        assert!(package.data_offset > 0);
    }

    #[test]
    fn empty_directory_is_a_valid_package() {
        let mut buf = Vec::new();
        push_sized(&mut buf, "PKGV0022");
        push_u32(&mut buf, 0);
        let package = must(Package::parse(&buf));
        assert!(package.is_empty());
        assert_eq!(package.data_offset, buf.len() as u64);
    }

    #[test]
    fn bad_magic_is_rejected() {
        let mut buf = Vec::new();
        push_sized(&mut buf, "ZIP00000");
        push_u32(&mut buf, 0);
        assert!(matches!(Package::parse(&buf), Err(Error::BadMagic { .. })));
    }

    #[test]
    fn parent_segments_are_rejected() {
        let mut buf = Vec::new();
        push_sized(&mut buf, "PKGV0001");
        push_u32(&mut buf, 1);
        push_sized(&mut buf, "materials/../../x.txt");
        push_u32(&mut buf, 0);
        push_u32(&mut buf, 0);
        assert!(matches!(Package::parse(&buf), Err(Error::UnsafePath(_))));
    }

    #[test]
    fn absolute_paths_are_rejected() {
        let mut buf = Vec::new();
        push_sized(&mut buf, "PKGV0001");
        push_u32(&mut buf, 1);
        push_sized(&mut buf, "/etc/passwd");
        push_u32(&mut buf, 0);
        push_u32(&mut buf, 0);
        assert!(matches!(Package::parse(&buf), Err(Error::UnsafePath(_))));
    }

    #[test]
    fn an_offset_past_the_archive_is_rejected() {
        let mut buf = Vec::new();
        push_sized(&mut buf, "PKGV0001");
        push_u32(&mut buf, 1);
        push_sized(&mut buf, "a.txt");
        push_u32(&mut buf, 0);
        push_u32(&mut buf, 4);
        assert!(matches!(Package::parse(&buf), Err(Error::BadRange { .. })));
    }

    #[test]
    fn a_short_header_is_an_eof_error() {
        assert!(matches!(
            Package::parse(&[1, 0, 0]),
            Err(Error::UnexpectedEof { .. })
        ));
    }

    #[test]
    fn a_path_must_be_utf8() {
        let mut buf = Vec::new();
        push_sized(&mut buf, "PKGV0001");
        push_u32(&mut buf, 1);
        push_u32(&mut buf, 2);
        buf.extend_from_slice(&[0xff, 0xff]);
        push_u32(&mut buf, 0);
        push_u32(&mut buf, 0);
        assert!(matches!(
            Package::parse(&buf),
            Err(Error::InvalidUtf8 { .. })
        ));
    }
}

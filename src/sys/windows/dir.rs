//! Directory listings with `FindFirstFileExW` and `FindNextFileW`.

use core::fmt;

use alloc_crate::sync::Arc;
use windows_sys::Win32::{
    Foundation::{
        ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_FILES, ERROR_PATH_NOT_FOUND,
        HANDLE, INVALID_HANDLE_VALUE,
    },
    Storage::FileSystem::{
        FIND_FIRST_EX_FLAGS, FIND_FIRST_EX_LARGE_FETCH, FindClose,
        FindExInfoBasic, FindExSearchNameMatch, FindFirstFileExW,
        FindNextFileW, WIN32_FIND_DATAW,
    },
};

use super::{
    fs::{FileAttr, FileType},
    os::{last_error, win_error},
    path::{DOT, NativePath, SEP},
};
use crate::{
    ffi::OsString,
    io,
    os::windows::ffi::OsStringExt,
    path::{Path, PathBuf},
};

/// An open search, closed on drop.
pub(crate) struct FindHandle(HANDLE);

// SAFETY: a search handle is a process-wide kernel32 handle, which any
// thread may use and close; `ReadDir` uses it through `&mut self` only.
unsafe impl Send for FindHandle {}
// SAFETY: see above; `&FindHandle` gives no access to the handle.
unsafe impl Sync for FindHandle {}

impl Drop for FindHandle {
    fn drop(&mut self) {
        // SAFETY: the handle is an open search that this value owns.
        let ok = unsafe { FindClose(self.0) };
        debug_assert!(ok != 0, "FindClose failed on an open search");
    }
}

/// Starts a search for `pattern`, a converted path, returning the first
/// match or the error code.
pub(crate) fn find_first(
    pattern: &[u16],
    flags: FIND_FIRST_EX_FLAGS,
) -> Result<(FindHandle, WIN32_FIND_DATAW), u32> {
    let mut data = WIN32_FIND_DATAW::default();
    // `FindExInfoBasic` skips the short name, which std does not use.
    // SAFETY: `pattern` is NUL-terminated, `data` is valid for writes of the
    // structure `FindExInfoBasic` fills in, and there is no search filter.
    let handle = unsafe {
        FindFirstFileExW(
            pattern.as_ptr(),
            FindExInfoBasic,
            (&raw mut data).cast(),
            FindExSearchNameMatch,
            core::ptr::null(),
            flags,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(last_error());
    }
    Ok((FindHandle(handle), data))
}

/// An iterator over the entries of a directory.
pub(crate) struct ReadDir {
    /// The search, or `None` once it ended.
    handle: Option<FindHandle>,
    /// The directory as given, to which the entries join their names.
    root: Arc<Path>,
    /// The match that `FindFirstFileExW` returned, until it is yielded.
    first: Option<WIN32_FIND_DATAW>,
}

/// An entry of a directory, with the metadata that the listing holds.
pub(crate) struct DirEntry {
    root: Arc<Path>,
    data: WIN32_FIND_DATAW,
}

/// Returns whether `std::path::Path::join` would put a separator between
/// `path`, which is not verbatim, and a relative name.
const fn needs_separator(path: &[u8]) -> bool {
    match path {
        [.., b'\\' | b'/'] => false,
        // A bare drive, whose relative paths start with the drive letter.
        [drive, b':'] => !drive.is_ascii_alphabetic(),
        _ => true,
    }
}

pub(crate) fn readdir(path: &Path) -> io::Result<ReadDir> {
    const STAR: u16 = b'*' as u16;

    let bytes = path.as_os_str().as_encoded_bytes();
    // `*` would make the empty path the current directory; like std, fail
    // as opening it fails.
    if bytes.is_empty() {
        return Err(win_error(ERROR_PATH_NOT_FOUND));
    }
    let mut native = NativePath::new();
    let joined;
    // std searches for `path.join("*")`. Joining normalizes verbatim
    // paths, so they take the same way; for the others it appends `*`,
    // after a separator if needed, which the conversion does in place.
    let pattern = if bytes.starts_with(br"\\?\") {
        joined = path.join("*");
        native.convert(joined.as_os_str())?
    } else {
        let suffix: &[u16] = if needs_separator(bytes) {
            &[SEP, STAR]
        } else {
            &[STAR]
        };
        native.convert_with(path.as_os_str(), suffix)?
    };
    // A larger buffer takes fewer system calls for large directories.
    let (handle, first) = match find_first(pattern, FIND_FIRST_EX_LARGE_FETCH) {
        Ok((handle, data)) => (Some(handle), Some(data)),
        // Nothing matched, in a directory that exists: a missing directory
        // fails with `ERROR_PATH_NOT_FOUND`.
        Err(ERROR_FILE_NOT_FOUND) => (None, None),
        Err(error) => return Err(win_error(error)),
    };
    Ok(ReadDir {
        handle,
        root: Arc::from(path),
        first,
    })
}

impl Iterator for ReadDir {
    type Item = io::Result<DirEntry>;

    fn next(&mut self) -> Option<Self::Item> {
        let handle = self.handle.as_ref()?.0;
        if let Some(first) = self.first.take() {
            if let Some(entry) = DirEntry::new(&self.root, &first) {
                return Some(Ok(entry));
            }
        }
        let mut data = WIN32_FIND_DATAW::default();
        // Each pass consumes one entry of the listing, so the loop ends at
        // the end of the listing if not at an entry to yield.
        loop {
            // SAFETY: the search is open, and `data` is valid for writes.
            if unsafe { FindNextFileW(handle, &raw mut data) } == 0 {
                let error = last_error();
                self.handle = None;
                return (error != ERROR_NO_MORE_FILES)
                    .then(|| Err(win_error(error)));
            }
            if let Some(entry) = DirEntry::new(&self.root, &data) {
                return Some(Ok(entry));
            }
        }
    }
}

impl fmt::Debug for ReadDir {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&*self.root, f)
    }
}

impl DirEntry {
    /// Wraps a listed entry, unless it is `.` or `..`.
    fn new(root: &Arc<Path>, data: &WIN32_FIND_DATAW) -> Option<Self> {
        match data.cFileName {
            [DOT, 0, ..] | [DOT, DOT, 0, ..] => None,
            _ => Some(Self {
                root: Arc::clone(root),
                data: *data,
            }),
        }
    }

    pub(crate) fn path(&self) -> PathBuf {
        self.root.join(self.file_name())
    }

    pub(crate) fn file_name(&self) -> OsString {
        let name = self.data.cFileName.split(|&unit| unit == 0).next();
        OsString::from_wide(name.unwrap_or_default())
    }

    #[allow(clippy::unnecessary_wraps)]
    pub(crate) const fn file_type(&self) -> io::Result<FileType> {
        // `FileType` checks the reserved field only for reparse points.
        Ok(FileType::new(
            self.data.dwFileAttributes,
            self.data.dwReserved0,
        ))
    }

    #[allow(clippy::unnecessary_wraps)]
    pub(crate) const fn metadata(&self) -> io::Result<FileAttr> {
        Ok(FileAttr::from_find_data(&self.data))
    }
}

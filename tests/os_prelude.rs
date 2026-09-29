//! `os::unix::prelude` and `os::windows::prelude` against std's: the same
//! names, re-exporting the extension modules' own items, which a glob import
//! brings into scope. Also `os::windows::raw` against std's.
//!
//! Each `via_prelude_*` helper takes its bound from the prelude and calls
//! the trait through its module, so it compiles only if both name the same
//! trait.

#![cfg(all(feature = "fs", feature = "path"))]

mod fs_util;

#[cfg(unix)]
mod unix {
    use litestd::{
        ffi::{OsStr, OsString},
        fs, io,
        os::{
            fd,
            unix::{self as ext, prelude},
        },
    };

    use super::fs_util::TempDir;

    fn via_prelude_as_fd<T: prelude::AsFd>(t: &T) -> fd::BorrowedFd<'_> {
        fd::AsFd::as_fd(t)
    }

    fn via_prelude_as_raw_fd<T: prelude::AsRawFd>(t: &T) -> fd::RawFd {
        fd::AsRawFd::as_raw_fd(t)
    }

    /// # Safety
    ///
    /// As for `FromRawFd::from_raw_fd`.
    unsafe fn via_prelude_from_raw_fd<T: prelude::FromRawFd>(
        raw: fd::RawFd,
    ) -> T {
        // SAFETY: the caller vouches for `raw`.
        unsafe { fd::FromRawFd::from_raw_fd(raw) }
    }

    fn via_prelude_into_raw_fd<T: prelude::IntoRawFd>(t: T) -> fd::RawFd {
        fd::IntoRawFd::into_raw_fd(t)
    }

    // Miri does not implement `openat`, which `read_dir` needs.
    #[cfg(not(miri))]
    fn via_prelude_dir_entry<T: prelude::DirEntryExt>(t: &T) -> u64 {
        ext::fs::DirEntryExt::ino(t)
    }

    fn via_prelude_file<T: prelude::FileExt>(
        t: &T,
        buf: &mut [u8],
    ) -> io::Result<()> {
        ext::fs::FileExt::read_exact_at(t, buf, 0)
    }

    fn via_prelude_file_type<T: prelude::FileTypeExt>(t: &T) -> bool {
        ext::fs::FileTypeExt::is_fifo(t)
    }

    fn via_prelude_metadata<T: prelude::MetadataExt>(t: &T) -> u64 {
        ext::fs::MetadataExt::ino(t)
    }

    fn via_prelude_open_options<T: prelude::OpenOptionsExt>(
        t: &mut T,
    ) -> &mut T {
        ext::fs::OpenOptionsExt::mode(t, 0o600)
    }

    fn via_prelude_permissions<T: prelude::PermissionsExt>(t: &T) -> u32 {
        ext::fs::PermissionsExt::mode(t)
    }

    fn via_prelude_os_str<T: ?Sized + prelude::OsStrExt>(t: &T) -> &[u8] {
        ext::ffi::OsStrExt::as_bytes(t)
    }

    fn via_prelude_os_string<T: prelude::OsStringExt>(t: T) -> Vec<u8> {
        ext::ffi::OsStringExt::into_vec(t)
    }

    #[cfg(feature = "thread")]
    fn via_prelude_join_handle<T: prelude::JoinHandleExt>(
        t: &T,
    ) -> ext::thread::RawPthread {
        ext::thread::JoinHandleExt::as_pthread_t(t)
    }

    #[test]
    fn prelude_items_are_the_extension_modules_items() {
        let dir = TempDir::new("prelude-items");
        let path = dir.join("file");
        std::fs::write(&path, b"items").unwrap();
        let mut options = fs::OpenOptions::new();
        let file = via_prelude_open_options(options.read(true))
            .open(&path)
            .unwrap();
        let mut buf = [0; 5];
        via_prelude_file(&file, &mut buf).unwrap();
        assert_eq!(&buf, b"items");
        let meta = file.metadata().unwrap();
        let real = std::fs::metadata(&path).unwrap();
        let ino = std::os::unix::fs::MetadataExt::ino(&real);
        assert_eq!(via_prelude_metadata(&meta), ino);
        assert_eq!(
            via_prelude_permissions(&meta.permissions()),
            std::os::unix::fs::PermissionsExt::mode(&real.permissions())
        );
        assert!(!via_prelude_file_type(&meta.file_type()));
        #[cfg(not(miri))]
        {
            let mut entries = fs::read_dir(dir.path()).unwrap();
            let entry = entries.next().unwrap().unwrap();
            assert_eq!(via_prelude_dir_entry(&entry), ino);
        }

        let raw: prelude::RawFd = via_prelude_as_raw_fd(&file);
        let borrowed: prelude::BorrowedFd<'_> = via_prelude_as_fd(&file);
        assert_eq!(via_prelude_as_raw_fd(&borrowed), raw);
        assert_eq!(via_prelude_into_raw_fd(file), raw);
        // SAFETY: the file gave up `raw`, which nothing else owns.
        let owned: prelude::OwnedFd = unsafe { via_prelude_from_raw_fd(raw) };
        assert_eq!(via_prelude_as_raw_fd(&owned), raw);
        let _: fd::OwnedFd = owned;

        let bytes = b"a\xffb";
        assert_eq!(
            via_prelude_os_str(<OsStr as prelude::OsStrExt>::from_bytes(bytes)),
            bytes
        );
        let os_string =
            <OsString as prelude::OsStringExt>::from_vec(bytes.to_vec());
        assert_eq!(via_prelude_os_string(os_string), bytes);

        #[cfg(feature = "thread")]
        {
            use std::os::unix::thread::JoinHandleExt as _;

            let lite = litestd::thread::spawn(|| {});
            let real = std::thread::spawn(|| {});
            let raw: std::os::unix::thread::RawPthread =
                via_prelude_join_handle(&lite);
            assert_ne!(raw, real.as_pthread_t());
            lite.join().unwrap();
            real.join().unwrap();
        }
    }

    #[test]
    fn a_glob_import_brings_the_extensions_into_scope() {
        use litestd::os::unix::prelude::*;

        let dir = TempDir::new("prelude-glob");
        let path = dir.join("file");
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        file.write_all_at(b"glob", 0).unwrap();
        let meta = file.metadata().unwrap();
        assert_eq!(meta.mode() & 0o777, meta.permissions().mode() & 0o777);
        assert_eq!(meta.permissions().mode() & 0o077, 0);
        assert!(!meta.file_type().is_fifo());
        // Miri does not implement `openat`, which `read_dir` needs.
        #[cfg(not(miri))]
        {
            let mut entries = fs::read_dir(dir.path()).unwrap();
            assert_eq!(entries.next().unwrap().unwrap().ino(), meta.ino());
        }
        let fd: RawFd = file.as_raw_fd();
        let borrowed: BorrowedFd<'_> = file.as_fd();
        assert_eq!(borrowed.as_raw_fd(), fd);
        let fd = file.into_raw_fd();
        // SAFETY: the file gave up `fd`, which nothing else owns.
        let owned = unsafe { OwnedFd::from_raw_fd(fd) };
        assert_eq!(owned.as_raw_fd(), fd);
        assert_eq!(OsStr::from_bytes(b"\xff").as_bytes(), b"\xff");
        assert_eq!(OsString::from_vec(vec![0xff]).into_vec(), [0xff]);
        #[cfg(feature = "thread")]
        {
            let handle = litestd::thread::spawn(|| {});
            let _: litestd::os::unix::thread::RawPthread =
                handle.as_pthread_t();
            handle.join().unwrap();
        }
    }
}

#[cfg(windows)]
mod windows {
    use core::ffi::c_void;

    use litestd::{
        ffi::{OsStr, OsString},
        fs, io,
        os::windows::{self as ext, prelude},
    };

    use super::fs_util::TempDir;

    fn via_prelude_as_handle<T: prelude::AsHandle>(
        t: &T,
    ) -> ext::io::BorrowedHandle<'_> {
        ext::io::AsHandle::as_handle(t)
    }

    fn via_prelude_as_raw_handle<T: prelude::AsRawHandle>(
        t: &T,
    ) -> ext::io::RawHandle {
        ext::io::AsRawHandle::as_raw_handle(t)
    }

    /// # Safety
    ///
    /// As for `FromRawHandle::from_raw_handle`.
    unsafe fn via_prelude_from_raw_handle<T: prelude::FromRawHandle>(
        raw: ext::io::RawHandle,
    ) -> T {
        // SAFETY: the caller vouches for `raw`.
        unsafe { ext::io::FromRawHandle::from_raw_handle(raw) }
    }

    fn via_prelude_into_raw_handle<T: prelude::IntoRawHandle>(
        t: T,
    ) -> ext::io::RawHandle {
        ext::io::IntoRawHandle::into_raw_handle(t)
    }

    fn via_prelude_file<T: prelude::FileExt>(
        t: &T,
        buf: &mut [u8],
    ) -> io::Result<usize> {
        ext::fs::FileExt::seek_read(t, buf, 0)
    }

    fn via_prelude_metadata<T: prelude::MetadataExt>(t: &T) -> u64 {
        ext::fs::MetadataExt::file_size(t)
    }

    fn via_prelude_open_options<T: prelude::OpenOptionsExt>(
        t: &mut T,
    ) -> &mut T {
        ext::fs::OpenOptionsExt::share_mode(t, 7)
    }

    fn via_prelude_os_str<T: ?Sized + prelude::OsStrExt>(t: &T) -> Vec<u16> {
        ext::ffi::OsStrExt::encode_wide(t).collect()
    }

    fn via_prelude_os_string<T: prelude::OsStringExt>(wide: &[u16]) -> T {
        ext::ffi::OsStringExt::from_wide(wide)
    }

    #[test]
    fn prelude_items_are_the_extension_modules_items() {
        let dir = TempDir::new("prelude-items");
        let path = dir.join("file");
        std::fs::write(&path, b"items").unwrap();
        let mut options = fs::OpenOptions::new();
        let file = via_prelude_open_options(options.read(true))
            .open(&path)
            .unwrap();
        let mut buf = [0; 5];
        assert_eq!(via_prelude_file(&file, &mut buf).unwrap(), 5);
        assert_eq!(&buf, b"items");
        assert_eq!(via_prelude_metadata(&file.metadata().unwrap()), 5);

        let raw: prelude::RawHandle = via_prelude_as_raw_handle(&file);
        let borrowed: prelude::BorrowedHandle<'_> =
            via_prelude_as_handle(&file);
        assert_eq!(via_prelude_as_raw_handle(&borrowed), raw);
        assert_eq!(via_prelude_into_raw_handle(file), raw);
        // SAFETY: the file gave up `raw`, which nothing else owns.
        let owned: prelude::OwnedHandle =
            unsafe { via_prelude_from_raw_handle(raw) };
        assert_eq!(via_prelude_as_raw_handle(&owned), raw);
        let raw = via_prelude_into_raw_handle(owned);
        // SAFETY: as above.
        let checked = unsafe { prelude::HandleOrInvalid::from_raw_handle(raw) };
        let checked: ext::io::HandleOrInvalid = checked;
        let owned = ext::io::OwnedHandle::try_from(checked).unwrap();
        assert_eq!(via_prelude_as_raw_handle(&owned), raw);

        let wide = [u16::from(b'a'), 0xd800, u16::from(b'b')];
        let os_string: OsString = via_prelude_os_string(&wide);
        let os_str: &OsStr = &os_string;
        assert_eq!(via_prelude_os_str(os_str), wide);
    }

    #[test]
    fn a_glob_import_brings_the_extensions_into_scope() {
        use litestd::os::windows::prelude::*;

        let dir = TempDir::new("prelude-glob");
        let path = dir.join("file");
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .share_mode(7)
            .open(&path)
            .unwrap();
        assert_eq!(file.seek_write(b"glob", 0).unwrap(), 4);
        assert_eq!(file.metadata().unwrap().file_size(), 4);
        let raw: RawHandle = file.as_raw_handle();
        let borrowed: BorrowedHandle<'_> = file.as_handle();
        assert_eq!(borrowed.as_raw_handle(), raw);
        let raw = file.into_raw_handle();
        // SAFETY: the file gave up `raw`, which nothing else owns.
        let checked = unsafe { HandleOrInvalid::from_raw_handle(raw) };
        let owned = OwnedHandle::try_from(checked).unwrap();
        assert_eq!(owned.as_raw_handle(), raw);
        drop(owned);
        let wide: Vec<u16> = OsStr::new("w").encode_wide().collect();
        assert_eq!(OsString::from_wide(&wide), "w");
    }

    #[test]
    fn raw_types_are_stds() {
        use litestd::os::windows::raw;

        let handle: raw::HANDLE = core::ptr::null_mut::<c_void>();
        let _: std::os::windows::raw::HANDLE = handle;
        let socket: raw::SOCKET = raw::SOCKET::MAX;
        let _: std::os::windows::raw::SOCKET = socket;
        assert_eq!(size_of::<raw::SOCKET>(), size_of::<usize>());
    }

    #[cfg(feature = "thread")]
    #[test]
    fn the_thread_extension_module_exists() {
        #[allow(
            unused_imports,
            reason = "std's module has no items either"
        )]
        use litestd::os::windows::thread;
    }
}

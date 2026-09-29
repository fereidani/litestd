//! `litestd::os::fd` against `std::os::fd`: ownership, borrowing, raw
//! conversions and layout.

#![cfg(all(unix, feature = "fs"))]

extern crate alloc;

mod fs_util;

use alloc::{rc::Rc, sync::Arc};
use core::{
    mem::size_of,
    panic::{RefUnwindSafe, UnwindSafe},
};

use fs_util::TempDir;
use litestd::{
    fs as lfs,
    io::{Read as _, Write as _},
    os::{
        fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd, RawFd},
        unix::io as unix_io,
    },
};

const fn assert_traits<T: Send + Sync + Unpin + UnwindSafe + RefUnwindSafe>() {}

/// Whether `fd` is open in this process.
fn is_open(fd: RawFd) -> bool {
    // SAFETY: `F_GETFD` touches no memory.
    unsafe { libc::fcntl(fd, libc::F_GETFD) != -1 }
}

/// Moves `fd` to a number far above the ones other tests use, so that no
/// concurrent test reuses the number once it is closed: 5000, or half the
/// soft limit on descriptors if that is lower, as Ubuntu's 1024 is.
#[allow(clippy::needless_pass_by_value, reason = "`fd` is closed here")]
fn high(fd: OwnedFd) -> OwnedFd {
    let floor = floor();
    // SAFETY: `F_DUPFD_CLOEXEC` touches no memory.
    let new =
        unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, floor) };
    assert!(new >= floor);
    // SAFETY: `new` is a fresh descriptor that nothing else owns.
    unsafe { OwnedFd::from_raw_fd(new) }
}

/// The lowest descriptor number for [`high`]; Miri has no limit to ask.
fn floor() -> RawFd {
    if cfg!(miri) {
        return 5000;
    }
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `limit` is valid for writes of an `rlimit`.
    let r = unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &raw mut limit) };
    assert_eq!(r, 0);
    RawFd::try_from(limit.rlim_cur / 2).map_or(5000, |half| half.min(5000))
}

#[test]
fn layout_and_traits_match_std() {
    assert_traits::<OwnedFd>();
    assert_traits::<BorrowedFd<'static>>();
    assert_eq!(size_of::<OwnedFd>(), size_of::<RawFd>());
    assert_eq!(size_of::<BorrowedFd<'_>>(), size_of::<RawFd>());
    assert_eq!(size_of::<RawFd>(), size_of::<std::os::fd::RawFd>());
    // std stores `None` as -1; stable Rust cannot declare that niche.
    assert_eq!(size_of::<Option<OwnedFd>>(), 2 * size_of::<RawFd>());
}

#[test]
fn owned_fds_close_exactly_once() {
    let dir = TempDir::new("owned");
    let file = lfs::File::create(dir.join("file")).unwrap();
    let fd = high(OwnedFd::from(file));
    let raw = fd.as_raw_fd();
    assert!(is_open(raw));
    let clone = fd.try_clone().unwrap();
    assert_ne!(clone.as_raw_fd(), raw);
    assert!(clone.as_raw_fd() >= 3);
    // SAFETY: `F_GETFD` touches no memory.
    let flags = unsafe { libc::fcntl(clone.as_raw_fd(), libc::F_GETFD) };
    assert_eq!(flags & libc::FD_CLOEXEC, libc::FD_CLOEXEC);
    drop(fd);
    assert!(!is_open(raw));
    let raw = clone.into_raw_fd();
    assert!(is_open(raw));
    // SAFETY: `raw` was just released by `into_raw_fd`.
    let fd = high(unsafe { OwnedFd::from_raw_fd(raw) });
    assert!(!is_open(raw));
    let raw = fd.as_raw_fd();
    drop(fd);
    assert!(!is_open(raw));
}

#[test]
fn files_convert_to_and_from_fds() {
    let dir = TempDir::new("convert");
    let path = dir.join("file");
    std::fs::write(&path, b"contents").unwrap();
    let file = lfs::File::open(&path).unwrap();
    let raw = file.as_raw_fd();
    assert_eq!(file.as_fd().as_raw_fd(), raw);
    let fd = OwnedFd::from(file);
    assert_eq!(fd.as_raw_fd(), raw);
    let mut file = lfs::File::from(fd);
    let mut text = String::new();
    file.read_to_string(&mut text).unwrap();
    assert_eq!(text, "contents");
    // One descriptor moves from litestd to std and back.
    let raw = file.into_raw_fd();
    // SAFETY: `raw` was just released by `into_raw_fd`.
    let std_file =
        unsafe { <std::fs::File as std::os::fd::FromRawFd>::from_raw_fd(raw) };
    let raw = std::os::fd::IntoRawFd::into_raw_fd(std_file);
    // SAFETY: `raw` was just released by `into_raw_fd`.
    let file = unsafe { lfs::File::from_raw_fd(raw) };
    assert_eq!(file.as_raw_fd(), raw);
    let mut writer = lfs::OpenOptions::new().append(true).open(&path).unwrap();
    writer.write_all(b"!").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"contents!");
}

/// Duplicates the descriptor of a std file into a litestd `OwnedFd`.
fn dup(file: &std::fs::File) -> OwnedFd {
    use std::os::fd::AsRawFd as _;
    // SAFETY: `F_DUPFD_CLOEXEC` touches no memory.
    let fd = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 3) };
    assert!(fd >= 3);
    // SAFETY: `fd` is a fresh descriptor that nothing else owns.
    unsafe { OwnedFd::from_raw_fd(fd) }
}

fn through<T: AsFd>(t: T) -> RawFd {
    t.as_fd().as_raw_fd()
}

#[test]
fn borrowing_and_smart_pointers() {
    let dir = TempDir::new("borrow");
    let file = lfs::File::create(dir.join("file")).unwrap();
    let raw = file.as_raw_fd();
    let borrowed: BorrowedFd<'_> = file.as_fd();
    let copy = borrowed;
    assert_eq!(copy.as_raw_fd(), raw);
    assert_eq!(copy.as_fd().as_raw_fd(), raw);
    // SAFETY: `file` keeps the descriptor open while `again` lives.
    let again = unsafe { BorrowedFd::borrow_raw(raw) };
    assert_eq!(again.as_raw_fd(), raw);
    let owned = again.try_clone_to_owned().unwrap();
    assert_ne!(owned.as_raw_fd(), raw);
    assert_eq!(through(&file), raw);
    let mut file = file;
    assert_eq!(through(&mut file), raw);
    let boxed = Box::new(file);
    assert_eq!(through(&boxed), raw);
    assert_eq!(boxed.as_raw_fd(), raw);
    let arc = Arc::new(*boxed);
    assert_eq!(through(&arc), raw);
    assert_eq!(arc.as_raw_fd(), raw);
    let rc = Rc::new(owned);
    assert_eq!(through(&rc), rc.as_raw_fd());
    let dyn_box: Box<dyn AsFd> =
        Box::new(dup(&std::fs::File::open(dir.join("file")).unwrap()));
    assert!(through(&dyn_box) >= 3);
    // Raw descriptors convert to themselves.
    assert_eq!(7.as_raw_fd(), 7);
    assert_eq!(7.into_raw_fd(), 7);
    // SAFETY: a `RawFd` owns nothing.
    assert_eq!(unsafe { RawFd::from_raw_fd(7) }, 7);
    // `os::unix::io` re-exports `os::fd`.
    let _: Option<unix_io::OwnedFd> = None::<OwnedFd>;
    let _: unix_io::RawFd = raw;
}

#[test]
fn debug_matches_std() {
    use std::os::fd::AsRawFd as _;
    let dir = TempDir::new("fd-debug");
    let file = lfs::File::create(dir.join("file")).unwrap();
    let fd = OwnedFd::from(file);
    let raw = fd.as_raw_fd();
    assert_eq!(format!("{fd:?}"), format!("OwnedFd {{ fd: {raw} }}"));
    assert_eq!(
        format!("{:?}", fd.as_fd()),
        format!("BorrowedFd {{ fd: {raw} }}")
    );
    // The same text as std's.
    let real = std::fs::File::open(dir.join("file")).unwrap();
    let real = std::os::fd::OwnedFd::from(real);
    assert_eq!(
        format!("{real:?}"),
        format!("OwnedFd {{ fd: {} }}", real.as_raw_fd())
    );
}

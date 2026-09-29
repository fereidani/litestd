//! Spawning with `posix_spawn`, which glibc and musl implement with
//! `CLONE_VFORK`, FreeBSD with `vfork` and macOS with a system call, sparing
//! the copy of the parent's page tables that `fork` makes.

#[cfg(target_env = "gnu")]
use core::sync::atomic::AtomicU8;
#[cfg(any(target_env = "gnu", target_os = "freebsd"))]
use core::{
    ffi::c_void,
    ptr,
    sync::atomic::{AtomicPtr, Ordering},
};
use core::{
    ffi::{CStr, c_char, c_int},
    mem::MaybeUninit,
};

use crate::{io, sys::os::cvt};

/// Adds a working directory change to the file actions.
type AddChdir = unsafe extern "C" fn(
    *mut libc::posix_spawn_file_actions_t,
    *const c_char,
) -> c_int;

/// What `posix_spawnp` needs; everything is kept alive by the caller.
pub(super) struct Spawn<'a> {
    /// The program, found in this process's `PATH` if it has no slash.
    pub(super) program: *const c_char,
    /// The arguments, ended by null.
    pub(super) argv: *const *const c_char,
    pub(super) envp: *const *const c_char,
    pub(super) cwd: Option<&'a CStr>,
    /// The descriptors for the standard streams; `-1` inherits.
    pub(super) stdio: &'a [c_int; 3],
    pub(super) pgroup: Option<libc::pid_t>,
}

impl Spawn<'_> {
    /// Spawns the child, or returns `None` if the C library cannot do what
    /// is asked, and `fork` must.
    pub(super) fn run(&self) -> io::Result<Option<libc::pid_t>> {
        if !reports_exec_errors() {
            return Ok(None);
        }
        let chdir = match self.cwd {
            Some(cwd) => match add_chdir() {
                Some(add) => Some((add, cwd)),
                None => return Ok(None),
            },
            None => None,
        };
        let mut attr_storage = MaybeUninit::uninit();
        // SAFETY: `attr_storage` is valid for writes of the attributes.
        cvt_nz(unsafe {
            libc::posix_spawnattr_init(attr_storage.as_mut_ptr())
        })?;
        let mut attr = Attr(&mut attr_storage);
        let mut actions_storage = MaybeUninit::uninit();
        // SAFETY: `actions_storage` is valid for writes of the actions.
        cvt_nz(unsafe {
            libc::posix_spawn_file_actions_init(actions_storage.as_mut_ptr())
        })?;
        let mut actions = Actions(&mut actions_storage);
        self.add_actions(&mut actions, chdir)?;
        attr.configure(self.pgroup)?;
        let mut pid = 0;
        // SAFETY: the attributes and actions are initialized, `program` is a
        // C string, and `argv` and `envp` are null-terminated arrays of C
        // strings; `posix_spawnp` only reads them.
        cvt_nz(unsafe {
            libc::posix_spawnp(
                &raw mut pid,
                self.program,
                actions.0.as_ptr(),
                attr.0.as_ptr(),
                self.argv.cast(),
                self.envp.cast(),
            )
        })?;
        Ok(Some(pid))
    }

    /// Adds the stream and working directory changes.
    fn add_actions(
        &self,
        actions: &mut Actions<'_>,
        chdir: Option<(AddChdir, &CStr)>,
    ) -> io::Result<()> {
        let actions = actions.0.as_mut_ptr();
        let streams =
            [libc::STDIN_FILENO, libc::STDOUT_FILENO, libc::STDERR_FILENO];
        for (target, &fd) in streams.iter().zip(self.stdio) {
            if fd >= 0 {
                // SAFETY: `actions` is initialized; the call copies the
                // numbers.
                cvt_nz(unsafe {
                    libc::posix_spawn_file_actions_adddup2(actions, fd, *target)
                })?;
            }
        }
        if let Some((add, cwd)) = chdir {
            // SAFETY: `actions` is initialized and `cwd` is a C string, which
            // the function copies.
            cvt_nz(unsafe { add(actions, cwd.as_ptr()) })?;
        }
        Ok(())
    }
}

/// Initialized spawn attributes, destroyed when dropped.
struct Attr<'a>(&'a mut MaybeUninit<libc::posix_spawnattr_t>);

impl Attr<'_> {
    /// Resets `SIGPIPE` to its default action, as std does, and sets the
    /// process group. The signal mask is inherited, as in std.
    fn configure(&mut self, pgroup: Option<libc::pid_t>) -> io::Result<()> {
        let attr = self.0.as_mut_ptr();
        let mut flags = libc::POSIX_SPAWN_SETSIGDEF;
        if let Some(pgroup) = pgroup {
            flags |= libc::POSIX_SPAWN_SETPGROUP;
            // SAFETY: `attr` is initialized.
            cvt_nz(unsafe { libc::posix_spawnattr_setpgroup(attr, pgroup) })?;
        }
        let mut set = MaybeUninit::uninit();
        // SAFETY: `set` is valid for writes, and initialized by
        // `sigemptyset` before `sigaddset` and the attribute read it.
        unsafe {
            cvt(libc::sigemptyset(set.as_mut_ptr()))?;
            cvt(libc::sigaddset(set.as_mut_ptr(), libc::SIGPIPE))?;
            cvt_nz(libc::posix_spawnattr_setsigdefault(attr, set.as_ptr()))?;
        }
        // The flags fit: they are single bits below `c_short::MAX`.
        #[allow(clippy::cast_possible_truncation)]
        let flags = flags as libc::c_short;
        // SAFETY: `attr` is initialized.
        cvt_nz(unsafe { libc::posix_spawnattr_setflags(attr, flags) })
    }
}

impl Drop for Attr<'_> {
    fn drop(&mut self) {
        // SAFETY: the attributes were initialized and are not used again.
        let r = unsafe { libc::posix_spawnattr_destroy(self.0.as_mut_ptr()) };
        debug_assert_eq!(r, 0, "destroying initialized attributes succeeds");
    }
}

/// Initialized file actions, destroyed when dropped.
struct Actions<'a>(&'a mut MaybeUninit<libc::posix_spawn_file_actions_t>);

impl Drop for Actions<'_> {
    fn drop(&mut self) {
        // SAFETY: the actions were initialized and are not used again.
        let r = unsafe {
            libc::posix_spawn_file_actions_destroy(self.0.as_mut_ptr())
        };
        debug_assert_eq!(r, 0, "destroying initialized actions succeeds");
    }
}

/// Converts the error number that the `posix_spawn` functions return.
fn cvt_nz(r: c_int) -> io::Result<()> {
    if r == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(r))
    }
}

/// Whether `posix_spawn` reports why an `exec` failed. glibc does from
/// 2.24; earlier, the child just exits with 127, so std forks instead.
#[cfg(target_env = "gnu")]
fn reports_exec_errors() -> bool {
    /// Not yet checked, capable, or not.
    const UNKNOWN: u8 = 0;
    const YES: u8 = 1;
    const NO: u8 = 2;
    // Relaxed suffices: the flag publishes no other data, and racing threads
    // compute the same answer.
    static STATE: AtomicU8 = AtomicU8::new(UNKNOWN);
    match STATE.load(Ordering::Relaxed) {
        YES => true,
        NO => false,
        _ => {
            let version = crate::sys::os::glibc_version();
            let yes = version.is_some_and(|v| v >= (2, 24));
            STATE.store(if yes { YES } else { NO }, Ordering::Relaxed);
            yes
        }
    }
}

#[cfg(any(
    target_env = "musl",
    target_vendor = "apple",
    target_os = "freebsd"
))]
const fn reports_exec_errors() -> bool {
    true
}

/// `posix_spawn_file_actions_addchdir_np`, which glibc has from 2.29 and
/// FreeBSD from 13.1, or the same function under the name POSIX.1-2024
/// gives it, looked up at run time as std does: linking it would stop the
/// program from starting on older systems. A static program finds neither.
#[cfg(any(target_env = "gnu", target_os = "freebsd"))]
fn add_chdir() -> Option<AddChdir> {
    /// Marks the lookup as not done yet; `dlsym` never returns it.
    const UNKNOWN: *mut c_void = ptr::dangling_mut();
    // Relaxed suffices: the address is immutable once found, and racing
    // threads find the same one.
    static FOUND: AtomicPtr<c_void> = AtomicPtr::new(UNKNOWN);
    let mut found = FOUND.load(Ordering::Relaxed);
    if found == UNKNOWN {
        let names = [
            c"posix_spawn_file_actions_addchdir_np",
            c"posix_spawn_file_actions_addchdir",
        ];
        found = ptr::null_mut();
        for name in names {
            // SAFETY: `name` is a C string; `RTLD_DEFAULT` searches the
            // loaded objects.
            found = unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr()) };
            if !found.is_null() {
                break;
            }
        }
        FOUND.store(found, Ordering::Relaxed);
    }
    if found.is_null() {
        return None;
    }
    // SAFETY: the C library defines both names as functions of this type.
    Some(unsafe { core::mem::transmute::<*mut c_void, AddChdir>(found) })
}

/// musl has the function from 1.1.24, older than any Rust target's.
#[cfg(target_env = "musl")]
#[allow(clippy::unnecessary_wraps, reason = "glibc's may find none")]
fn add_chdir() -> Option<AddChdir> {
    Some(libc::posix_spawn_file_actions_addchdir_np)
}

/// macOS has the function from 10.15; the libc crate lacks it.
#[cfg(target_vendor = "apple")]
#[allow(clippy::unnecessary_wraps, reason = "glibc's may find none")]
fn add_chdir() -> Option<AddChdir> {
    unsafe extern "C" {
        fn posix_spawn_file_actions_addchdir_np(
            actions: *mut libc::posix_spawn_file_actions_t,
            path: *const c_char,
        ) -> c_int;
    }
    Some(posix_spawn_file_actions_addchdir_np)
}

//! Misuse that std answers with a panic or a hang aborts the process. Each
//! test runs itself again in a child process, marked by an environment
//! variable, where it takes the aborting path.

// WebAssembly has no child processes.
#![cfg(all(feature = "thread", not(target_family = "wasm")))]

use core::{cell::RefCell, time::Duration};
use std::{
    env, io,
    process::{Command, ExitStatus, Stdio},
    sync::mpsc,
    time::Instant,
};

use litestd::{thread, thread_local};

const CHILD: &str = "LITESTD_THREAD_ABORT_CHILD";

fn in_child() -> bool {
    env::var_os(CHILD).is_some()
}

/// Runs the test `name` of this binary in a child process that takes the
/// aborting path. Returns `None` if the child was still running after a
/// generous timeout, and kills it.
fn run_child(name: &str) -> io::Result<Option<ExitStatus>> {
    let mut child = Command::new(env::current_exe()?)
        .args(["--exact", name, "--test-threads=1"])
        .env(CHILD, "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(30) {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    child.kill()?;
    child.wait()?;
    Ok(None)
}

/// Checks that the child aborted, rather than hanging, passing, or failing
/// with a panic.
fn assert_aborted(status: Option<ExitStatus>) {
    assert!(status.is_some(), "the child hung instead of aborting");
    if let Some(status) = status {
        assert!(!status.success(), "the child exited normally");
        assert_ne!(status.code(), Some(101), "the child panicked");
    }
}

/// Blocks the child's test thread until another thread ends the process.
fn wait_for_abort() -> ! {
    loop {
        thread::park();
    }
}

#[test]
#[cfg_attr(miri, ignore = "Miri cannot spawn processes")]
fn joining_the_calling_thread_aborts() {
    if in_child() {
        let (tx, rx) = mpsc::channel::<thread::JoinHandle<()>>();
        let handle = thread::spawn(move || {
            let own = rx.recv().unwrap();
            own.join().unwrap();
        });
        tx.send(handle).unwrap();
        wait_for_abort();
    }
    assert_aborted(run_child("joining_the_calling_thread_aborts").unwrap());
}

#[test]
#[cfg_attr(miri, ignore = "Miri cannot spawn processes")]
fn joining_the_calling_thread_from_a_destructor_aborts() {
    struct JoinOnDrop(RefCell<Option<thread::JoinHandle<()>>>);
    impl Drop for JoinOnDrop {
        fn drop(&mut self) {
            if let Some(own) = self.0.take() {
                own.join().unwrap();
            }
        }
    }
    thread_local!(static OWN: JoinOnDrop = const {
        JoinOnDrop(RefCell::new(None))
    });

    if in_child() {
        let (tx, rx) = mpsc::channel::<thread::JoinHandle<()>>();
        let handle = thread::spawn(move || {
            let own = rx.recv().unwrap();
            OWN.with(|slot| *slot.0.borrow_mut() = Some(own));
        });
        tx.send(handle).unwrap();
        wait_for_abort();
    }
    assert_aborted(
        run_child("joining_the_calling_thread_from_a_destructor_aborts")
            .unwrap(),
    );
}

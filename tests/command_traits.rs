//! The `process` types implement exactly the traits of their std
//! counterparts, auto traits included, and format and decode as std does.
//! Nothing here spawns a process, so Miri runs it too.

#![cfg(feature = "process")]
#![allow(clippy::unwrap_used, reason = "a failure fails the test")]

use core::{
    error::Error,
    fmt::{Debug, Display},
    hash::Hash,
    panic::{RefUnwindSafe, UnwindSafe},
};

use litestd::process::{ExitCode, Termination};

/// Evaluates to whether `$ty` implements `$tr`. The inherent constant takes
/// precedence over the trait's, but exists only where the bound holds.
macro_rules! implements {
    ($ty:ty: $tr:path) => {{
        struct Probe<T: ?Sized>(core::marker::PhantomData<T>);
        #[allow(dead_code)]
        trait Fallback {
            const YES: bool = false;
        }
        impl<T: ?Sized> Fallback for Probe<T> {}
        #[allow(dead_code)]
        impl<T: ?Sized + $tr> Probe<T> {
            const YES: bool = true;
        }
        <Probe<$ty>>::YES
    }};
}

/// Asserts that `$lite` and `$std` agree on the auto traits and on the
/// common derivable and formatting traits.
macro_rules! same_traits {
    ($lite:ty, $std:ty) => {
        same_traits!(
            $lite, $std: Send, Sync, Unpin, UnwindSafe, RefUnwindSafe, Clone,
            Copy, Debug, Default, Display, Eq, Error, Hash, Ord, PartialEq,
            PartialOrd, Iterator, ExactSizeIterator, DoubleEndedIterator,
            core::iter::FusedIterator,
        );
    };
    ($lite:ty, $std:ty: $($tr:path),+ $(,)?) => {
        $(
            assert_eq!(
                implements!($lite: $tr),
                implements!($std: $tr),
                "{} and {} disagree on {}",
                stringify!($lite),
                stringify!($std),
                stringify!($tr),
            );
        )+
    };
}

#[test]
fn exit_code_and_termination() {
    same_traits!(ExitCode, std::process::ExitCode);
    assert_eq!(
        format!("{:?}", ExitCode::SUCCESS),
        format!("{:?}", std::process::ExitCode::SUCCESS)
    );
    assert_eq!(
        format!("{:#?}", ExitCode::from(7)),
        format!("{:#?}", std::process::ExitCode::from(7))
    );
    assert_eq!(ExitCode::default(), ExitCode::SUCCESS);
    assert_ne!(ExitCode::SUCCESS, ExitCode::FAILURE);
    assert_eq!(ExitCode::from(1), ExitCode::FAILURE);
    assert_eq!(().report(), ExitCode::SUCCESS);
    assert_eq!(ExitCode::from(3).report(), ExitCode::from(3));
    assert_eq!(Ok::<(), u8>(()).report(), ExitCode::SUCCESS);
    assert_eq!(
        Ok::<ExitCode, u8>(ExitCode::from(9)).report(),
        ExitCode::from(9)
    );
    assert_eq!(
        Err::<(), &str>("printed to stderr").report(),
        ExitCode::FAILURE
    );
}

/// std implements `Termination` for `!`, which stable Rust names only as a
/// function's return type.
#[test]
fn never_is_termination() {
    trait Output {
        type Type;
    }
    impl<R> Output for fn() -> R {
        type Type = R;
    }
    #[allow(unreachable_code, reason = "no value of `!` exists")]
    fn report(never: <fn() -> ! as Output>::Type) -> ExitCode {
        never.report()
    }
    let _: fn(_) -> ExitCode = report;
}

#[cfg(feature = "command")]
mod command {
    use litestd::process::{
        Child, ChildStderr, ChildStdin, ChildStdout, Command, CommandArgs,
        CommandEnvs, ExitStatus, Output, Stdio,
    };

    use super::*;

    #[test]
    fn types_have_std_traits() {
        same_traits!(Command, std::process::Command);
        same_traits!(Child, std::process::Child);
        same_traits!(ChildStdin, std::process::ChildStdin);
        same_traits!(ChildStdout, std::process::ChildStdout);
        same_traits!(ChildStderr, std::process::ChildStderr);
        same_traits!(Output, std::process::Output);
        same_traits!(ExitStatus, std::process::ExitStatus);
        same_traits!(Stdio, std::process::Stdio);
        same_traits!(CommandArgs<'static>, std::process::CommandArgs<'static>);
        same_traits!(CommandEnvs<'static>, std::process::CommandEnvs<'static>);
    }

    #[test]
    fn pipes_read_and_write_like_std() {
        use litestd::io::{Read, Write};
        assert!(implements!(ChildStdin: Write));
        assert!(implements!(&'static ChildStdin: Write));
        assert!(implements!(ChildStdout: Read));
        assert!(implements!(ChildStderr: Read));
        assert!(!implements!(ChildStdin: Read));
        assert!(!implements!(ChildStdout: Write));
        assert!(!implements!(&'static ChildStdout: Read));
        for (lite, std) in [
            (
                implements!(Stdio: From<ChildStdin>),
                implements!(std::process::Stdio: From<std::process::ChildStdin>),
            ),
            (implements!(Stdio: From<ChildStdout>), true),
            (implements!(Stdio: From<ChildStderr>), true),
        ] {
            assert_eq!(lite, std);
        }
        #[cfg(feature = "fs")]
        assert!(implements!(Stdio: From<litestd::fs::File>));
    }

    #[cfg(unix)]
    #[test]
    fn unix_descriptor_traits_match_std() {
        use litestd::os::fd::{AsFd, AsRawFd, FromRawFd, IntoRawFd, OwnedFd};
        macro_rules! fd_traits {
            ($($lite:ident $std:ident),*) => {$(
                assert!(implements!($lite: AsFd) && implements!($lite: AsRawFd));
                assert!(implements!($lite: IntoRawFd) && implements!($lite: From<OwnedFd>));
                assert!(implements!(OwnedFd: From<$lite>));
                assert_eq!(
                    implements!($lite: FromRawFd),
                    implements!(std::process::$std: std::os::fd::FromRawFd),
                );
            )*};
        }
        fd_traits!(ChildStdin ChildStdin, ChildStdout ChildStdout, ChildStderr ChildStderr);
        assert!(
            implements!(Stdio: FromRawFd) && implements!(Stdio: From<OwnedFd>)
        );
        assert!(!implements!(Child: AsRawFd));
    }

    /// Builds a command twice, for litestd and std, with the same calls.
    macro_rules! both {
        ($program:expr $(, $method:ident($($arg:expr),*))*) => {{
            let mut lite = Command::new($program);
            let mut std = std::process::Command::new($program);
            $(lite.$method($($arg),*); std.$method($($arg),*);)*
            (lite, std)
        }};
    }

    #[test]
    fn debug_matches_std() {
        let cases = [
            both!(
                "echo",
                arg("a b"),
                arg("it's"),
                arg("h\u{e9}llo"),
                arg("tab\t")
            ),
            both!(
                "echo",
                env("TERM", "dumb"),
                env_remove("TZ"),
                current_dir("/tmp")
            ),
            both!("x", env_clear(), env("A", "1"), env_remove("B")),
            both!(
                "p",
                env("B", "2"),
                env("A", "1"),
                env_remove("C"),
                env("a", "3")
            ),
        ];
        for (lite, std) in cases {
            assert_eq!(format!("{lite:?}"), format!("{std:?}"));
            assert_eq!(
                format!("{:?}", lite.get_args()),
                format!("{:?}", std.get_args())
            );
            assert_eq!(
                format!("{:?}", lite.get_envs()),
                format!("{:?}", std.get_envs())
            );
        }
        assert_eq!(
            format!("{:?}", Stdio::piped()),
            format!("{:?}", std::process::Stdio::piped())
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_debug_matches_std() {
        use std::os::unix::process::CommandExt as _;

        use litestd::os::unix::process::CommandExt;
        let (mut lite, mut std) =
            both!("echo", arg("x"), current_dir("/tmp"), env("K", "v"));
        lite.arg0("zero").process_group(0).uid(1).gid(2);
        std.arg0("zero").process_group(0).uid(1).gid(2);
        assert_eq!(format!("{lite:?}"), format!("{std:?}"));
        lite.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        std.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit());
        assert_eq!(format!("{lite:#?}"), format!("{std:#?}"));
    }

    #[test]
    fn getters_match_std() {
        let (lite, std) = both!(
            "prog",
            arg("1"),
            args(["2", "3"]),
            env("Z", "z"),
            env("A", "a"),
            env_remove("M"),
            current_dir("dir")
        );
        assert_eq!(lite.get_program().to_str(), std.get_program().to_str());
        let lite_args: Vec<_> = lite.get_args().map(|a| a.to_str()).collect();
        let std_args: Vec<_> = std.get_args().map(|a| a.to_str()).collect();
        assert_eq!(lite_args, std_args);
        let mut args = lite.get_args();
        assert_eq!((args.len(), args.size_hint()), (3, (3, Some(3))));
        args.next();
        assert_eq!(args.len(), 2);
        let lite_envs: Vec<_> = lite
            .get_envs()
            .map(|(k, v)| (k.to_str(), v.map(|v| v.to_str())))
            .collect();
        let std_envs: Vec<_> = std
            .get_envs()
            .map(|(k, v)| (k.to_str(), v.map(|v| v.to_str())))
            .collect();
        assert_eq!(lite_envs, std_envs);
        assert_eq!(lite.get_envs().len(), 3);
        assert_eq!(
            lite.get_current_dir().and_then(|d| d.to_str()),
            Some("dir")
        );
        let (mut lite, _) = both!("p", env("A", "1"));
        lite.env_clear();
        assert_eq!(lite.get_envs().len(), 0);
        lite.env_remove("A");
        assert_eq!(lite.get_envs().len(), 0);
        assert!(Command::new("p").get_current_dir().is_none());
    }

    #[test]
    fn exit_status_and_output_format_like_std() {
        assert_eq!(
            format!("{:?}", ExitStatus::default()),
            format!("{:?}", std::process::ExitStatus::default())
        );
        assert_eq!(
            ExitStatus::default().to_string(),
            std::process::ExitStatus::default().to_string()
        );
        assert!(ExitStatus::default().success());
        let lite = Output {
            status: ExitStatus::default(),
            stdout: b"hi".to_vec(),
            stderr: vec![0xff],
        };
        let std = std::process::Output {
            status: std::process::ExitStatus::default(),
            stdout: b"hi".to_vec(),
            stderr: vec![0xff],
        };
        assert_eq!(format!("{lite:?}"), format!("{std:?}"));
        assert_eq!(format!("{lite:#?}"), format!("{std:#?}"));
        assert_eq!(lite.clone(), lite);
    }

    #[cfg(unix)]
    #[test]
    fn unix_wait_statuses_decode_like_std() {
        use std::os::unix::process::ExitStatusExt as _;

        use litestd::os::unix::process::ExitStatusExt;
        let statuses = [
            0,
            1 << 8,
            42 << 8,
            255 << 8,
            9,
            15,
            0x80 | 6,
            0x137f,
            0x407f,
            0xffff,
            0x10000,
            0xff,
            0x7f,
            -1,
            i32::MIN,
        ];
        for raw in statuses {
            let lite = ExitStatus::from_raw(raw);
            let std = std::process::ExitStatus::from_raw(raw);
            assert_eq!(lite.code(), std.code(), "{raw:#x}");
            assert_eq!(lite.success(), std.success(), "{raw:#x}");
            assert_eq!(lite.signal(), std.signal(), "{raw:#x}");
            assert_eq!(lite.core_dumped(), std.core_dumped(), "{raw:#x}");
            assert_eq!(lite.stopped_signal(), std.stopped_signal(), "{raw:#x}");
            assert_eq!(lite.continued(), std.continued(), "{raw:#x}");
            assert_eq!(lite.into_raw(), raw);
            assert_eq!(lite.to_string(), std.to_string(), "{raw:#x}");
            assert_eq!(format!("{lite:?}"), format!("{std:?}"), "{raw:#x}");
        }
    }
}

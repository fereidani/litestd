//! The entry point shared by the size probes.

#![no_std]

/// Defines the program's entry point, which runs the body and exits with
/// status 0: std's `main`, or with `lite` the C runtime's. A failed check
/// in the body panics, which ends the process with a failure status.
#[macro_export]
macro_rules! main {
    ($($body:tt)*) => {
        #[cfg(feature = "lite")]
        #[unsafe(no_mangle)]
        extern "C" fn main(
            _argc: core::ffi::c_int,
            _argv: *const *const core::ffi::c_char,
        ) -> core::ffi::c_int {
            run();
            0
        }

        #[cfg(not(feature = "lite"))]
        fn main() {
            run();
        }

        fn run() {
            $($body)*
        }
    };
}

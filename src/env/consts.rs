//! Constants associated with the current target.

/// Defines [`ARCH`] for each architecture name, as `target_arch` spells it.
macro_rules! arch {
    ($($name:literal)*) => {
        $(
            /// A string describing the architecture of the CPU that is
            /// currently in use, such as `"x86_64"` or `"aarch64"`.
            #[cfg(target_arch = $name)]
            pub const ARCH: &str = $name;
        )*
        #[cfg(not(any($(target_arch = $name),*)))]
        compile_error!("litestd does not know the name of this architecture");
    };
}

arch!(
    "aarch64" "arm" "arm64ec" "csky" "hexagon" "loongarch64"
    "m68k" "mips" "mips32r6" "mips64" "mips64r6" "powerpc" "powerpc64"
    "riscv32" "riscv64" "s390x" "sparc" "sparc64" "wasm32" "x86" "x86_64"
);

/// A string describing the family of the operating system: `"unix"` or
/// `"windows"`, or an empty string on WebAssembly, as in std.
pub const FAMILY: &str = if cfg!(windows) {
    "windows"
} else if cfg!(target_family = "wasm") {
    ""
} else {
    "unix"
};

/// A string describing the specific operating system in use, such as
/// `"linux"`, `"macos"`, `"freebsd"` or `"windows"`, or an empty string on
/// WebAssembly, WASI included, as in std.
pub const OS: &str = if cfg!(windows) {
    "windows"
} else if cfg!(target_family = "wasm") {
    ""
} else if cfg!(target_os = "android") {
    "android"
} else if cfg!(target_os = "macos") {
    "macos"
} else if cfg!(target_os = "freebsd") {
    "freebsd"
} else if cfg!(target_os = "netbsd") {
    "netbsd"
} else if cfg!(target_os = "openbsd") {
    "openbsd"
} else if cfg!(target_os = "dragonfly") {
    "dragonfly"
} else {
    "linux"
};

/// Specifies the filename prefix used for shared libraries on this
/// platform: `"lib"` or an empty string.
pub const DLL_PREFIX: &str = if cfg!(any(windows, target_family = "wasm")) {
    ""
} else {
    "lib"
};

/// Specifies the filename suffix used for shared libraries on this
/// platform, such as `".so"`, `".dylib"` or `".dll"`.
pub const DLL_SUFFIX: &str = if cfg!(windows) {
    ".dll"
} else if cfg!(target_family = "wasm") {
    ".wasm"
} else if cfg!(target_vendor = "apple") {
    ".dylib"
} else {
    ".so"
};

/// Specifies the file extension used for shared libraries on this platform
/// that goes after the dot, such as `"so"`, `"dylib"` or `"dll"`.
pub const DLL_EXTENSION: &str = if cfg!(windows) {
    "dll"
} else if cfg!(target_family = "wasm") {
    "wasm"
} else if cfg!(target_vendor = "apple") {
    "dylib"
} else {
    "so"
};

/// Specifies the filename suffix used for executable binaries on this
/// platform: `".exe"`, `".wasm"` or an empty string.
pub const EXE_SUFFIX: &str = if cfg!(windows) {
    ".exe"
} else if cfg!(target_family = "wasm") {
    ".wasm"
} else {
    ""
};

/// Specifies the file extension used for executable binaries on this
/// platform: `"exe"`, `"wasm"` or an empty string.
pub const EXE_EXTENSION: &str = if cfg!(windows) {
    "exe"
} else if cfg!(target_family = "wasm") {
    "wasm"
} else {
    ""
};

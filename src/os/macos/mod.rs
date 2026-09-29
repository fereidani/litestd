//! macOS-specific definitions.

pub mod fs {
    //! macOS-specific extensions to primitives in the [`fs`](crate::fs)
    //! module.

    pub use crate::os::darwin::fs::{FileTimesExt, MetadataExt};
}

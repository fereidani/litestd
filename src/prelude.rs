//! The litestd prelude: `std::prelude`'s modules, whose `alloc` items need
//! the `alloc` feature. A `no_std` crate does not get them implicitly;
//! glob-import one, such as `litestd::prelude::rust_2024::*`.

/// The first version of the prelude.
pub mod v1 {
    pub use core::prelude::v1::*;

    #[cfg(feature = "alloc")]
    pub use alloc_crate::{
        borrow::ToOwned,
        boxed::Box,
        string::{String, ToString},
        vec::Vec,
    };
}

/// The 2015 edition of the prelude.
pub mod rust_2015 {
    pub use super::v1::*;
}

/// The 2018 edition of the prelude.
pub mod rust_2018 {
    pub use super::v1::*;
}

/// The 2021 edition of the prelude.
pub mod rust_2021 {
    pub use core::prelude::rust_2021::*;

    // The rest of `v1` is already part of core's edition prelude.
    #[cfg(feature = "alloc")]
    pub use super::v1::{Box, String, ToOwned, ToString, Vec};
}

/// The 2024 edition of the prelude.
pub mod rust_2024 {
    pub use core::prelude::rust_2024::*;

    // The rest of `v1` is already part of core's edition prelude.
    #[cfg(feature = "alloc")]
    pub use super::v1::{Box, String, ToOwned, ToString, Vec};
}

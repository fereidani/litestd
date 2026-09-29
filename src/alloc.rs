//! Memory allocation APIs.
//!
//! A `no_std` binary registers [`System`] as its global allocator with the
//! `global-allocator` feature, or with its own `#[global_allocator]` static.

pub use core::alloc::{GlobalAlloc, Layout, LayoutError};

#[cfg(feature = "alloc")]
pub use alloc_crate::alloc::{
    alloc, alloc_zeroed, dealloc, handle_alloc_error, realloc,
};

use crate::sys;

/// The default memory allocator provided by the operating system.
///
/// It supports every [`Layout`], whatever the alignment: `malloc` and
/// `posix_memalign` on Unix, the process heap on Windows.
#[derive(Clone, Copy, Debug, Default)]
pub struct System;

// SAFETY: each method forwards to the backend function of the same name,
// which implements that method's `GlobalAlloc` contract.
unsafe impl GlobalAlloc for System {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller upholds `GlobalAlloc::alloc`'s contract.
        unsafe { sys::alloc::alloc(layout) }
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller upholds `GlobalAlloc::alloc_zeroed`'s contract.
        unsafe { sys::alloc::alloc_zeroed(layout) }
    }

    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller upholds `GlobalAlloc::dealloc`'s contract.
        unsafe { sys::alloc::dealloc(ptr, layout) }
    }

    #[inline]
    unsafe fn realloc(
        &self,
        ptr: *mut u8,
        layout: Layout,
        new_size: usize,
    ) -> *mut u8 {
        // SAFETY: the caller upholds `GlobalAlloc::realloc`'s contract.
        unsafe { sys::alloc::realloc(ptr, layout, new_size) }
    }
}

/// The global allocator under the `global-allocator` feature.
#[cfg(feature = "global-allocator")]
#[global_allocator]
static GLOBAL: System = System;

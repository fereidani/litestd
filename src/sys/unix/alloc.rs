//! The system allocator on top of `malloc`. Each function has the contract
//! of the `GlobalAlloc` method of the same name.

use core::{alloc::Layout, ffi::c_void, ptr};

/// The alignment `malloc` guarantees for blocks at least this large. A
/// smaller block may be less aligned (jemalloc gives an 8-byte block 8-byte
/// alignment), so the fast paths also require `align <= size`.
const MIN_ALIGN: usize = if cfg!(any(
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "loongarch64",
    target_arch = "mips64",
    target_arch = "mips64r6",
    target_arch = "riscv64",
    target_arch = "s390x",
    target_arch = "sparc64",
)) {
    16
} else {
    8
};

/// Returns whether `malloc(size)` is aligned enough for `align`.
const fn malloc_aligns(align: usize, size: usize) -> bool {
    align <= MIN_ALIGN && align <= size
}

/// Allocates with `posix_memalign`, for alignments `malloc` does not
/// guarantee.
#[cold]
fn aligned_alloc(layout: Layout) -> *mut u8 {
    // Older macOS versions return misaligned blocks for larger alignments;
    // std refuses them on every Apple system, and so does litestd.
    if cfg!(target_vendor = "apple") && layout.align() > 1 << 31 {
        return ptr::null_mut();
    }
    let mut block = ptr::null_mut();
    // `posix_memalign` requires a power of two that is a multiple of the
    // pointer size; `Layout` guarantees the power of two.
    let align = layout.align().max(size_of::<usize>());
    // SAFETY: `block` is valid for writes, and `align` meets the
    // requirement above.
    let r =
        unsafe { libc::posix_memalign(&raw mut block, align, layout.size()) };
    if r == 0 {
        block.cast()
    } else {
        ptr::null_mut()
    }
}

/// # Safety
///
/// `layout` must have a nonzero size.
pub(crate) unsafe fn alloc(layout: Layout) -> *mut u8 {
    if malloc_aligns(layout.align(), layout.size()) {
        // SAFETY: `malloc` has no preconditions.
        unsafe { libc::malloc(layout.size()) }.cast()
    } else {
        aligned_alloc(layout)
    }
}

/// # Safety
///
/// `layout` must have a nonzero size.
pub(crate) unsafe fn alloc_zeroed(layout: Layout) -> *mut u8 {
    if malloc_aligns(layout.align(), layout.size()) {
        // SAFETY: `calloc` has no preconditions.
        unsafe { libc::calloc(layout.size(), 1) }.cast()
    } else {
        let block = aligned_alloc(layout);
        if !block.is_null() {
            // SAFETY: `block` is a fresh allocation of `layout.size()` bytes.
            unsafe { block.write_bytes(0, layout.size()) };
        }
        block
    }
}

/// # Safety
///
/// `block` must have been allocated by this module with `layout`.
pub(crate) unsafe fn dealloc(block: *mut u8, _layout: Layout) {
    // SAFETY: `block` came from `malloc`, `calloc`, `realloc` or
    // `posix_memalign`, all of which `free` accepts.
    unsafe { libc::free(block.cast::<c_void>()) }
}

/// # Safety
///
/// `block` must have been allocated by this module with `layout`, and
/// `new_size` must be nonzero and must not overflow `isize` when rounded up
/// to `layout.align()`.
pub(crate) unsafe fn realloc(
    block: *mut u8,
    layout: Layout,
    new_size: usize,
) -> *mut u8 {
    if malloc_aligns(layout.align(), new_size) {
        // SAFETY: `block` is live and came from one of the allocation
        // functions `realloc` accepts, `posix_memalign` included on glibc,
        // musl and macOS. The result is aligned because `new_size >= align`.
        return unsafe { libc::realloc(block.cast(), new_size) }.cast();
    }
    // `realloc` would drop the alignment: move the block by hand. A layout
    // that cannot exist gets null, as `GlobalAlloc::realloc` allows.
    let Ok(new_layout) = Layout::from_size_align(new_size, layout.align())
    else {
        return ptr::null_mut();
    };
    // SAFETY: `new_size` is nonzero.
    let new_block = unsafe { alloc(new_layout) };
    if !new_block.is_null() {
        // SAFETY: both blocks are live, distinct and valid for the copied
        // length, and the caller hands `block` over to be freed.
        unsafe {
            ptr::copy_nonoverlapping(
                block,
                new_block,
                layout.size().min(new_size),
            );
            dealloc(block, layout);
        }
    }
    new_block
}

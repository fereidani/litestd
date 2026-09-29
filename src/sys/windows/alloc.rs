//! The system allocator on the process heap; each function has the contract
//! of its `GlobalAlloc` namesake. A layout aligned beyond `MIN_ALIGN` gets a
//! larger block, whose start is stored just below the aligned pointer.

use core::{alloc::Layout, ffi::c_void, ptr};

use windows_sys::Win32::{
    Foundation::HANDLE,
    System::Memory::{
        GetProcessHeap, HEAP_FLAGS, HEAP_ZERO_MEMORY, HeapAlloc, HeapFree,
        HeapReAlloc,
    },
};

/// `MEMORY_ALLOCATION_ALIGNMENT`: the alignment of every `HeapAlloc` block.
const MIN_ALIGN: usize = 2 * size_of::<usize>();

fn heap() -> HANDLE {
    // SAFETY: `GetProcessHeap` has no preconditions.
    unsafe { GetProcessHeap() }
}

/// Allocates a block for `layout`, zeroed if `flags` is `HEAP_ZERO_MEMORY`.
fn allocate(layout: Layout, flags: HEAP_FLAGS) -> *mut u8 {
    let heap = heap();
    // A null process heap only means the call failed.
    if heap.is_null() {
        return ptr::null_mut();
    }
    if layout.align() <= MIN_ALIGN {
        // SAFETY: `heap` is the process heap and `flags` holds only
        // `HEAP_ZERO_MEMORY` or nothing.
        return unsafe { HeapAlloc(heap, flags, layout.size()) }.cast();
    }
    // `Layout` keeps `size` rounded up to `align` within `isize::MAX`, so
    // this sum fits in a `usize`; failing that, report no memory.
    let Some(total) = layout.size().checked_add(layout.align()) else {
        return ptr::null_mut();
    };
    // SAFETY: as above.
    let block = unsafe { HeapAlloc(heap, flags, total) }.cast::<u8>();
    if block.is_null() {
        return block;
    }
    debug_assert_eq!(block.addr() % MIN_ALIGN, 0);
    // `block` is `MIN_ALIGN`-aligned and `align > MIN_ALIGN`, so `offset`
    // lies in `MIN_ALIGN..=align`: room for the header below the aligned
    // pointer, and for `size` bytes above it within `total`.
    let offset = layout.align() - (block.addr() & (layout.align() - 1));
    // SAFETY: `offset <= align`, so the result stays inside the block.
    let aligned = unsafe { block.add(offset) };
    // SAFETY: the header occupies `aligned - size_of::<usize>()..aligned`,
    // inside the block because `offset >= MIN_ALIGN >= size_of::<usize>()`,
    // and it is pointer-aligned because `aligned` is `align`-aligned.
    #[allow(clippy::cast_ptr_alignment, reason = "aligned, see above")]
    unsafe {
        aligned.cast::<*mut u8>().sub(1).write(block);
    }
    aligned
}

/// Returns the start of the heap block behind `ptr`.
///
/// # Safety
///
/// `ptr` must have been returned by [`allocate`] with `layout`.
const unsafe fn block_of(ptr: *mut u8, layout: Layout) -> *mut u8 {
    if layout.align() <= MIN_ALIGN {
        ptr
    } else {
        // SAFETY: `allocate` stored the block's start just below `ptr`, which
        // is pointer-aligned because `ptr` is `align`-aligned.
        #[allow(clippy::cast_ptr_alignment, reason = "aligned, see above")]
        unsafe {
            ptr.cast::<*mut u8>().sub(1).read()
        }
    }
}

/// # Safety
///
/// `layout` must have a nonzero size.
pub(crate) unsafe fn alloc(layout: Layout) -> *mut u8 {
    allocate(layout, 0)
}

/// # Safety
///
/// `layout` must have a nonzero size.
pub(crate) unsafe fn alloc_zeroed(layout: Layout) -> *mut u8 {
    // `HEAP_ZERO_MEMORY` zeroes the whole block, whatever the alignment.
    allocate(layout, HEAP_ZERO_MEMORY)
}

/// # Safety
///
/// `ptr` must have been allocated by this module with `layout`.
pub(crate) unsafe fn dealloc(ptr: *mut u8, layout: Layout) {
    // SAFETY: the caller guarantees `ptr` and `layout` match.
    let block = unsafe { block_of(ptr, layout) };
    // SAFETY: `block` is a live block of the process heap.
    let ok = unsafe { HeapFree(heap(), 0, block.cast::<c_void>()) };
    // Freeing a live block cannot fail.
    debug_assert!(ok != 0);
}

/// # Safety
///
/// `ptr` must have been allocated by this module with `layout`, and
/// `new_size` must be nonzero and must not overflow `isize` when rounded up
/// to `layout.align()`.
pub(crate) unsafe fn realloc(
    ptr: *mut u8,
    layout: Layout,
    new_size: usize,
) -> *mut u8 {
    if layout.align() <= MIN_ALIGN {
        // SAFETY: `ptr` is a live block of the process heap, and
        // `HeapReAlloc` keeps `MIN_ALIGN` alignment.
        return unsafe {
            HeapReAlloc(heap(), 0, ptr.cast::<c_void>(), new_size)
        }
        .cast();
    }
    // `HeapReAlloc` may move the block to another offset from the alignment,
    // so over-aligned blocks move by hand. A `new_size` that overflows
    // `isize` when rounded up, which the caller rules out, returns null.
    let Ok(new_layout) = Layout::from_size_align(new_size, layout.align())
    else {
        return ptr::null_mut();
    };
    let new_ptr = allocate(new_layout, 0);
    if !new_ptr.is_null() {
        // SAFETY: both blocks are live, distinct and valid for the copied
        // length, and the caller hands `ptr` over to be freed.
        unsafe {
            ptr::copy_nonoverlapping(ptr, new_ptr, layout.size().min(new_size));
            dealloc(ptr, layout);
        }
    }
    new_ptr
}

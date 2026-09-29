//! `litestd::alloc::System` honors every `Layout`.
//!
//! The allocator is also this test binary's global allocator, so the test
//! harness and every `Vec` and `String` below run on it.

use core::{ptr, slice};

use litestd::alloc::{GlobalAlloc, Layout, System};

#[global_allocator]
static GLOBAL: System = System;

/// Alignments from byte-aligned to far beyond what `malloc` guarantees.
const ALIGNS: &[usize] = if cfg!(miri) {
    &[1, 16, 64, 4096]
} else {
    &[1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 4096, 65536, 1 << 20]
};

/// Sizes around each alignment, including sizes below it, which `malloc`
/// may align less.
fn sizes(align: usize) -> [usize; 7] {
    [
        1,
        3,
        align.saturating_sub(1).max(1),
        align,
        align + 1,
        3 * align + 5,
        1000,
    ]
}

#[cfg(test)]
fn layout(size: usize, align: usize) -> Layout {
    Layout::from_size_align(size, align).unwrap()
}

/// The pattern derived from `seed`: byte `i` is `seed + i`, wrapping, so
/// it repeats every 256 bytes. `fill` and `check` copy and compare whole
/// periods, which Miri runs far faster than single bytes.
#[allow(clippy::cast_possible_truncation, reason = "the pattern wraps")]
fn pattern(seed: u8) -> [u8; 256] {
    core::array::from_fn(|i| seed.wrapping_add(i as u8))
}

/// Fills `len` bytes at `block` with the pattern of `seed`.
///
/// # Safety
///
/// `block` must be valid for writes of `len` bytes.
unsafe fn fill(block: *mut u8, len: usize, seed: u8) {
    // SAFETY: the caller guarantees `block` is valid for `len` bytes.
    let bytes = unsafe { slice::from_raw_parts_mut(block, len) };
    let pattern = pattern(seed);
    for chunk in bytes.chunks_mut(pattern.len()) {
        chunk.copy_from_slice(&pattern[..chunk.len()]);
    }
}

/// Checks the pattern `fill` wrote.
///
/// # Safety
///
/// `block` must be valid for reads of `len` initialized bytes.
#[cfg(test)]
unsafe fn check(block: *const u8, len: usize, seed: u8) {
    // SAFETY: the caller guarantees `block` is valid for `len` bytes.
    let bytes = unsafe { slice::from_raw_parts(block, len) };
    let pattern = pattern(seed);
    for (n, chunk) in bytes.chunks(pattern.len()).enumerate() {
        assert_eq!(chunk, &pattern[..chunk.len()], "period {n} of {len} bytes");
    }
}

#[test]
fn alloc_honors_size_and_alignment() {
    for &align in ALIGNS {
        for size in sizes(align) {
            let layout = layout(size, align);
            // SAFETY: the size is nonzero; the block is freed with `layout`.
            unsafe {
                let block = System.alloc(layout);
                assert!(!block.is_null(), "{layout:?}");
                assert_eq!(block.addr() % align, 0, "{layout:?}");
                fill(block, size, 7);
                check(block, size, 7);
                System.dealloc(block, layout);
            }
        }
    }
}

#[test]
fn alloc_zeroed_zeroes_every_class() {
    for &align in ALIGNS {
        for size in sizes(align) {
            let layout = layout(size, align);
            // SAFETY: as above.
            unsafe {
                // Dirty a block first, so that a reused block would show.
                let dirty = System.alloc(layout);
                fill(dirty, size, 0xA5);
                System.dealloc(dirty, layout);

                let block = System.alloc_zeroed(layout);
                assert!(!block.is_null(), "{layout:?}");
                assert_eq!(block.addr() % align, 0, "{layout:?}");
                let bytes = slice::from_raw_parts(block, size);
                assert!(bytes.iter().all(|&b| b == 0), "{layout:?}");
                System.dealloc(block, layout);
            }
        }
    }
}

#[test]
fn realloc_preserves_contents_and_alignment() {
    for &align in ALIGNS {
        for old_size in sizes(align) {
            for new_size in sizes(align) {
                let old = layout(old_size, align);
                // SAFETY: sizes are nonzero and fit `isize` after rounding;
                // each block is freed with the layout it now has.
                unsafe {
                    let block = System.alloc(old);
                    assert!(!block.is_null());
                    fill(block, old_size, 3);
                    let moved = System.realloc(block, old, new_size);
                    assert!(!moved.is_null(), "{old:?} -> {new_size}");
                    assert_eq!(
                        moved.addr() % align,
                        0,
                        "{old:?} -> {new_size}"
                    );
                    check(moved, old_size.min(new_size), 3);
                    System.dealloc(moved, layout(new_size, align));
                }
            }
        }
    }
}

#[test]
fn realloc_crosses_the_malloc_boundary() {
    // A block smaller than its alignment comes from `posix_memalign` on
    // Unix; growing it lets `realloc` take over, and shrinking it back
    // needs the aligned path again.
    let small = layout(4, 8);
    // SAFETY: as above.
    unsafe {
        let block = System.alloc(small);
        fill(block, 4, 9);
        let grown = System.realloc(block, small, 64);
        assert_eq!(grown.addr() % 8, 0);
        check(grown, 4, 9);
        fill(grown, 64, 11);
        let shrunk = System.realloc(grown, layout(64, 8), 4);
        assert_eq!(shrunk.addr() % 8, 0);
        check(shrunk, 4, 11);
        System.dealloc(shrunk, small);
    }
}

#[cfg(not(miri))]
#[test]
#[cfg_attr(
    target_family = "wasm",
    ignore = "a wasm32 memory can hold isize::MAX bytes"
)]
fn huge_requests_fail_cleanly() {
    for &align in ALIGNS {
        // The largest size `Layout` accepts for this alignment.
        let size = isize::MAX as usize - (align - 1);
        let huge = layout(size, align);
        // SAFETY: the size is nonzero; a non-null block would be freed.
        unsafe {
            let block = System.alloc(huge);
            assert!(block.is_null(), "{huge:?}");
            let block = System.alloc_zeroed(huge);
            assert!(block.is_null(), "{huge:?}");

            let small = layout(align, align);
            let block = System.alloc(small);
            assert!(!block.is_null());
            fill(block, align, 1);
            // A failed reallocation leaves the block untouched and owned by
            // the caller.
            let moved = System.realloc(block, small, size);
            assert!(moved.is_null(), "{small:?} -> {size}");
            check(block, align, 1);
            System.dealloc(block, small);
        }
    }
}

/// A type whose alignment `malloc` does not guarantee.
#[repr(align(128))]
struct Padded(u8);

#[test]
fn global_allocator_serves_collections() {
    let mut v: Vec<u64> = Vec::new();
    for i in 0..10_000 {
        v.push(i);
    }
    assert_eq!(v.iter().sum::<u64>(), 10_000 * 9_999 / 2);
    v.shrink_to_fit();
    let s: String = (b'a'..=b'z').cycle().take(1000).map(char::from).collect();
    assert_eq!(s.len(), 1000);

    let boxed = Box::new(Padded(5));
    assert_eq!(ptr::from_ref(&*boxed).addr() % 128, 0);
    assert_eq!(boxed.0, 5);
}

#[test]
#[cfg_attr(
    all(target_family = "wasm", not(target_feature = "atomics")),
    ignore = "WebAssembly without atomics has no threads"
)]
fn concurrent_allocation() {
    let threads: u8 = if cfg!(miri) { 2 } else { 8 };
    std::thread::scope(|s| {
        for t in 0..threads {
            s.spawn(move || {
                let mut blocks = Vec::new();
                for i in 0..if cfg!(miri) { 20 } else { 2000 } {
                    blocks.push(vec![t; (i % 97) + 1]);
                }
                assert!(blocks.iter().all(|b| b.iter().all(|&x| x == t)));
            });
        }
    });
}

#[test]
fn system_traits() {
    fn assert_traits<
        T: Clone + Copy + Default + core::fmt::Debug + Send + Sync,
    >() {
    }
    assert_traits::<System>();
    assert_eq!(format!("{System:?}"), "System");
}

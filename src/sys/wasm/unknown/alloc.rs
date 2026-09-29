//! The allocator of a module without an OS, on the memory that
//! `memory.grow` adds. Blocks of up to `SMALL_MAX` bytes come in power-of-two
//! classes, each aligned to its size: a freed block goes on its class's
//! free list, which the next request of the class takes from. Larger blocks
//! are runs of whole pages, reused first fit from a list in address order
//! that merges neighbors; a run at the end of memory grows in place.
//! WebAssembly cannot return memory to the host, so the heap only grows.
//! Threads, with the `atomics` target feature, take turns through a lock.
//! Each function has the contract of the `GlobalAlloc` method of the same
//! name.

use core::{
    alloc::Layout,
    arch::wasm32,
    cell::UnsafeCell,
    cmp,
    ops::{Deref, DerefMut},
    ptr,
};
#[cfg(target_feature = "atomics")]
use core::{
    cell::Cell,
    sync::atomic::{
        AtomicBool, AtomicUsize,
        Ordering::{Acquire, Relaxed, Release},
    },
};

/// The size of a WebAssembly page, which `memory.grow` adds.
const PAGE: usize = 65536;

/// The smallest class: room for a free list's link, and 16-byte alignment.
const MIN: usize = 16;

/// The largest class; larger blocks are runs of pages.
const SMALL_MAX: usize = 16384;

/// The number of classes, from `MIN` to `SMALL_MAX` bytes.
const CLASSES: usize =
    (SMALL_MAX.trailing_zeros() - MIN.trailing_zeros()) as usize + 1;

/// The offset of a free run's page count, after its link.
const COUNT: usize = size_of::<usize>();

/// What a block of `layout` takes: a class of at most `SMALL_MAX` bytes,
/// which also covers the alignment, or a number of pages, whose run is
/// placed to meet an alignment beyond a page.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Block {
    Small(usize),
    Pages(usize),
}

impl Block {
    fn of(layout: Layout) -> Self {
        let small = cmp::max(layout.size(), layout.align());
        if small <= SMALL_MAX {
            Self::Small(small.max(MIN).next_power_of_two())
        } else {
            Self::Pages(layout.size().div_ceil(PAGE))
        }
    }
}

/// The heap. Addresses are byte offsets into the module's memory, 0 for
/// none, which no block has: the first page holds the data and stack.
struct Heap {
    /// The page that small blocks are carved from, and the bytes of it
    /// handed out, `PAGE` before the first. Offsets within the page keep
    /// every sum below the end of a full 4 GiB memory.
    page: usize,
    used: usize,
    /// The first free block of each class, linked through its first word.
    free: [usize; CLASSES],
    /// The free run of pages with the lowest address. Each holds the next
    /// one and its page count in its first two words.
    runs: usize,
}

/// The heap, and with threads the lock that `Locked` holds.
struct Global {
    heap: UnsafeCell<Heap>,
    #[cfg(target_feature = "atomics")]
    locked: AtomicBool,
}

// SAFETY: without the `atomics` feature the module runs one thread; with
// it, `Locked` lends the heap to one thread at a time.
unsafe impl Sync for Global {}

static HEAP: Global = Global {
    heap: UnsafeCell::new(Heap {
        page: 0,
        used: PAGE,
        free: [0; CLASSES],
        runs: 0,
    }),
    #[cfg(target_feature = "atomics")]
    locked: AtomicBool::new(false),
};

/// The heap, lent to one allocator call until dropped.
struct Locked(&'static mut Heap);

impl Locked {
    /// Takes the heap, waiting while another thread holds it.
    ///
    /// # Safety
    ///
    /// The calling thread must not hold it already: the allocator functions
    /// take it once each and never call each other while they hold it.
    unsafe fn take() -> Self {
        // A browser's main thread may not wait, so every thread spins, as
        // std's allocator does. The holder runs only allocator code, which
        // never waits, so it releases the lock and the loop ends.
        #[cfg(target_feature = "atomics")]
        {
            while HEAP.locked.swap(true, Acquire) {
                while HEAP.locked.load(Relaxed) {
                    core::hint::spin_loop();
                }
            }
            see_growth();
        }
        // SAFETY: one thread runs, or this one holds the lock, and the
        // caller holds no other reference.
        Self(unsafe { &mut *HEAP.heap.get() })
    }
}

/// How often the memory grew, and how often the calling thread has seen it
/// grow.
#[cfg(target_feature = "atomics")]
static GROWN: AtomicUsize = AtomicUsize::new(0);
#[cfg(target_feature = "atomics")]
#[thread_local]
static SEEN: Cell<usize> = Cell::new(0);

/// Brings the calling thread's view of the memory's size up to date once
/// another thread grew it, before a block on the new pages reaches the
/// caller. V8 checks `memory.fill` and `memory.copy` against a size that
/// each thread keeps and updates later, but updates it at once on a
/// `memory.grow`, even of no pages.
#[cfg(target_feature = "atomics")]
fn see_growth() {
    // The lock orders every growth before this read.
    let grown = GROWN.load(Relaxed);
    if SEEN.get() != grown {
        wasm32::memory_grow::<0>(0);
        SEEN.set(grown);
    }
}

impl Deref for Locked {
    type Target = Heap;

    fn deref(&self) -> &Heap {
        self.0
    }
}

impl DerefMut for Locked {
    fn deref_mut(&mut self) -> &mut Heap {
        self.0
    }
}

impl Drop for Locked {
    fn drop(&mut self) {
        // The thread sees its own growth.
        #[cfg(target_feature = "atomics")]
        {
            SEEN.set(GROWN.load(Relaxed));
            HEAP.locked.store(false, Release);
        }
    }
}

/// Reads the word at `addr`.
///
/// # Safety
///
/// `addr` must be the address of a free block or run, which holds its links
/// in its first words.
unsafe fn read(addr: usize) -> usize {
    // SAFETY: the memory at `addr` belongs to the heap, which the host gave
    // the module, and is aligned to at least 16 bytes.
    unsafe { ptr::with_exposed_provenance::<usize>(addr).read() }
}

/// Writes `value` to the word at `addr`.
///
/// # Safety
///
/// As for `read`.
unsafe fn write(addr: usize, value: usize) {
    // SAFETY: as in `read`.
    unsafe { ptr::with_exposed_provenance_mut::<usize>(addr).write(value) };
}

/// Adds `pages` pages to the module's memory and returns their address.
/// With threads, the caller holds the lock.
fn grow(pages: usize) -> Option<usize> {
    match wasm32::memory_grow::<0>(pages) {
        usize::MAX => None,
        old => {
            #[cfg(target_feature = "atomics")]
            GROWN.fetch_add(1, Relaxed);
            old.checked_mul(PAGE)
        }
    }
}

impl Heap {
    fn alloc(&mut self, block: Block, align: usize) -> Option<usize> {
        match block {
            Block::Small(size) => self.pop(size).or_else(|| self.carve(size)),
            Block::Pages(pages) if align <= PAGE => {
                self.take_run(pages).or_else(|| grow(pages))
            }
            Block::Pages(pages) => self.take_aligned(pages, align),
        }
    }

    /// Takes `pages` pages aligned to `align`, a power of two beyond a page,
    /// from a run with room to slide them there, and frees the rest of the
    /// run before and after them.
    fn take_aligned(&mut self, pages: usize, align: usize) -> Option<usize> {
        let slack = align / PAGE - 1;
        let total = pages.checked_add(slack)?;
        let run = self.take_run(total).or_else(|| grow(total))?;
        // Within the run, as `slack` pages cover any distance to the next
        // multiple of `align`.
        let start = run.next_multiple_of(align);
        let head = (start - run) / PAGE;
        if head > 0 {
            // SAFETY: the pages before `start` belong to the taken run.
            unsafe { self.free_run(run, head) };
        }
        if slack > head {
            // SAFETY: as above, for the pages after the block, which end
            // inside memory.
            unsafe { self.free_run(start + pages * PAGE, slack - head) };
        }
        Some(start)
    }

    /// # Safety
    ///
    /// `addr` must be a live block of `block` from this heap.
    unsafe fn dealloc(&mut self, addr: usize, block: Block) {
        match block {
            // SAFETY: the caller gives the block back.
            Block::Small(size) => unsafe { self.push(addr, size) },
            // SAFETY: as above.
            Block::Pages(pages) => unsafe { self.free_run(addr, pages) },
        }
    }

    /// The free runs around page `page`: the last one below it and the first
    /// one at or above it, 0 where there is none. Pages, not bytes, so that
    /// the end of a full memory does not overflow.
    fn around(&self, page: usize) -> (usize, usize) {
        let (mut prev, mut next) = (0, self.runs);
        // Each pass moves along the list, which ends with 0.
        while next != 0 && next / PAGE < page {
            // SAFETY: `next` is a free run, which holds its links.
            (prev, next) = (next, unsafe { read(next) });
        }
        (prev, next)
    }

    /// Frees the run of `pages` pages at `addr`, merged with the free runs
    /// right before and after it.
    ///
    /// # Safety
    ///
    /// The run must be free and handed out nowhere.
    unsafe fn free_run(&mut self, addr: usize, mut pages: usize) {
        let (prev, mut next) = self.around(addr / PAGE);
        if next != 0 && addr / PAGE + pages == next / PAGE {
            // SAFETY: `next` is a free run, which holds its links.
            unsafe { (pages, next) = (pages + read(next + COUNT), read(next)) };
        }
        if prev != 0 {
            // SAFETY: as above, for `prev`.
            let before = unsafe { read(prev + COUNT) };
            if prev / PAGE + before == addr / PAGE {
                // SAFETY: as above.
                unsafe {
                    write(prev, next);
                    write(prev + COUNT, before + pages);
                }
                return;
            }
        }
        // SAFETY: the caller gives the run up, and a page has room for two
        // words; `prev` is 0 or a free run.
        unsafe {
            write(addr, next);
            write(addr + COUNT, pages);
            self.link(prev, addr);
        }
    }

    /// Makes `run` follow `prev` in the list, or head it if `prev` is 0.
    ///
    /// # Safety
    ///
    /// `prev` must be 0 or a free run.
    unsafe fn link(&mut self, prev: usize, run: usize) {
        if prev == 0 {
            self.runs = run;
        } else {
            // SAFETY: the caller guarantees that `prev` is a free run.
            unsafe { write(prev, run) };
        }
    }

    /// Takes the first `pages` of the free run `run` of `count` pages, which
    /// follows `prev` and precedes `next` in the list; the rest stays free in
    /// its place.
    ///
    /// # Safety
    ///
    /// `run` must be a free run of `count` pages, at least `pages`, `prev` 0
    /// or the free run before it, and `next` the one after it.
    unsafe fn split(
        &mut self,
        (prev, run, next): (usize, usize, usize),
        count: usize,
        pages: usize,
    ) {
        debug_assert!(count >= pages);
        let rest = if count > pages {
            let tail = run + pages * PAGE;
            // SAFETY: the tail pages belong to the free run.
            unsafe {
                write(tail, next);
                write(tail + COUNT, count - pages);
            }
            tail
        } else {
            next
        };
        // SAFETY: the caller guarantees that `prev` is 0 or a free run.
        unsafe { self.link(prev, rest) };
    }

    /// The class list of blocks of `size` bytes.
    fn list(&mut self, size: usize) -> Option<&mut usize> {
        let index = size.trailing_zeros().checked_sub(MIN.trailing_zeros())?;
        self.free.get_mut(index as usize)
    }

    /// Takes a free block of `size` bytes.
    fn pop(&mut self, size: usize) -> Option<usize> {
        let head = self.list(size)?;
        let addr = *head;
        if addr == 0 {
            return None;
        }
        // SAFETY: `addr` heads the list of free blocks, which link to the
        // next one in their first word.
        *head = unsafe { read(addr) };
        Some(addr)
    }

    /// Puts the free block `addr` of `size` bytes on its class list.
    ///
    /// # Safety
    ///
    /// `addr` must be a block of `size` bytes, aligned to it, that nothing
    /// else uses.
    unsafe fn push(&mut self, addr: usize, size: usize) {
        if let Some(head) = self.list(size) {
            // SAFETY: the caller gives the block to the list.
            unsafe { write(addr, *head) };
            *head = addr;
        }
    }

    /// Hands out a block of `size` bytes from the rest of the current page,
    /// or from a new page if it does not fit. A page is aligned to every
    /// class, so an offset aligned to `size` gives an aligned block.
    fn carve(&mut self, size: usize) -> Option<usize> {
        let mut start = self.used.next_multiple_of(size);
        if start + size > PAGE {
            let page = self.take_run(1).or_else(|| grow(1))?;
            // SAFETY: the rest of the old page is free and handed out
            // nowhere.
            unsafe { self.release(self.page, self.used, PAGE) };
            (self.page, start) = (page, 0);
        } else {
            // SAFETY: as above, for the gap before `start`.
            unsafe { self.release(self.page, self.used, start) };
        }
        self.used = start + size;
        Some(self.page + start)
    }

    /// Puts the free bytes `from..to` of `page` on the class lists, as the
    /// largest blocks that fit, each aligned to its size.
    ///
    /// # Safety
    ///
    /// The bytes must be free and handed out nowhere. `from` and `to` must be
    /// multiples of `MIN`, at most `PAGE`.
    unsafe fn release(&mut self, page: usize, mut from: usize, to: usize) {
        debug_assert!(from % MIN == 0 && to % MIN == 0 && to <= PAGE);
        // Each pass takes a block of `MIN` bytes at least, so the loop ends
        // once `from` reaches `to`.
        while from < to {
            // The largest class that `from` is aligned to and that fits the
            // rest: `MIN` at least, as both ends are multiples of it.
            let size = 1
                << from
                    .trailing_zeros()
                    .min((to - from).ilog2())
                    .min(SMALL_MAX.trailing_zeros());
            // SAFETY: the caller gives the bytes up, and the block is aligned
            // to its size.
            unsafe { self.push(page + from, size) };
            from += size;
        }
    }

    /// Takes `pages` pages from the first free run large enough, leaving the
    /// rest of the run free in its place.
    fn take_run(&mut self, pages: usize) -> Option<usize> {
        let (mut prev, mut run) = (0, self.runs);
        // Each pass moves along the list, which ends with 0.
        while run != 0 {
            // SAFETY: `run` is a free run, which holds its links.
            let (next, count) = unsafe { (read(run), read(run + COUNT)) };
            if count >= pages {
                // SAFETY: `run` holds `count` pages, between `prev` and
                // `next`.
                unsafe { self.split((prev, run, next), count, pages) };
                return Some(run);
            }
            (prev, run) = (run, next);
        }
        None
    }

    /// Resizes the run of `old` pages at `addr` to `pages` in place: a
    /// shrunk run frees its tail, and a grown one takes the free run right
    /// after it and, at the end of memory, new pages. Returns whether it did.
    ///
    /// # Safety
    ///
    /// `addr` must be a live run of `old` pages from this heap.
    unsafe fn resize(&mut self, addr: usize, old: usize, pages: usize) -> bool {
        if let Some(tail) = old.checked_sub(pages) {
            // SAFETY: the caller gives up the tail of its run.
            unsafe { self.free_run(addr + pages * PAGE, tail) };
            return true;
        }
        let end = addr / PAGE + old;
        let (prev, run) = self.around(end);
        let (next, free) = if run != 0 && run / PAGE == end {
            // SAFETY: `run` is a free run, which holds its links.
            unsafe { (read(run), read(run + COUNT)) }
        } else {
            (run, 0)
        };
        let need = pages - old;
        if free >= need {
            // SAFETY: `run`, right after the block, holds `free` pages,
            // between `prev` and `next`.
            unsafe { self.split((prev, run, next), free, need) };
            return true;
        }
        // The block and its free neighbor reach the end of memory, which new
        // pages extend. The host, or a thread that grew the memory unseen
        // here, may put the new pages elsewhere, where they go free.
        let at = end + free;
        if at != wasm32::memory_size::<0>() {
            return false;
        }
        let Some(start) = grow(need - free) else {
            return false;
        };
        if start / PAGE != at {
            // SAFETY: the new pages belong to this heap and nothing uses
            // them.
            unsafe { self.free_run(start, need - free) };
            return false;
        }
        if free != 0 {
            // SAFETY: the block takes all of `run`, as above.
            unsafe { self.split((prev, run, next), free, free) };
        }
        true
    }
}

/// # Safety
///
/// `layout` must have a nonzero size.
pub(crate) unsafe fn alloc(layout: Layout) -> *mut u8 {
    let block = Block::of(layout);
    // SAFETY: this thread holds the heap nowhere else.
    let addr = unsafe { Locked::take() }.alloc(block, layout.align());
    addr.map_or(ptr::null_mut(), ptr::with_exposed_provenance_mut)
}

/// # Safety
///
/// As for [`alloc`].
pub(crate) unsafe fn alloc_zeroed(layout: Layout) -> *mut u8 {
    // SAFETY: the caller upholds `alloc`'s contract.
    let block = unsafe { alloc(layout) };
    if !block.is_null() {
        // SAFETY: the block holds `layout.size()` bytes. Reused blocks hold
        // old data, so every block is cleared.
        unsafe { ptr::write_bytes(block, 0, layout.size()) };
    }
    block
}

/// # Safety
///
/// `block` must come from this allocator with `layout`, and not have been
/// freed.
pub(crate) unsafe fn dealloc(block: *mut u8, layout: Layout) {
    let kind = Block::of(layout);
    // SAFETY: this thread holds the heap nowhere else, and the caller gives
    // back a live block of `layout`, which `alloc` gave `kind`.
    unsafe { Locked::take().dealloc(block.expose_provenance(), kind) };
}

/// # Safety
///
/// As for [`dealloc`], and `new_size` must be nonzero and not overflow
/// `isize` when rounded up to `layout.align()`.
pub(crate) unsafe fn realloc(
    block: *mut u8,
    layout: Layout,
    new_size: usize,
) -> *mut u8 {
    // SAFETY: the caller guarantees that the new layout is valid.
    let new =
        unsafe { Layout::from_size_align_unchecked(new_size, layout.align()) };
    let (from, to) = (Block::of(layout), Block::of(new));
    // A block that already has the room keeps its place.
    if from == to {
        return block;
    }
    if let (Block::Pages(old), Block::Pages(pages)) = (from, to) {
        let addr = block.expose_provenance();
        // SAFETY: this thread holds the heap nowhere else, and the caller
        // gives a live run of `old` pages.
        if unsafe { Locked::take().resize(addr, old, pages) } {
            return block;
        }
    }
    // SAFETY: the caller upholds `alloc`'s contract for `new`.
    let moved = unsafe { alloc(new) };
    if !moved.is_null() {
        // SAFETY: both blocks hold the smaller size, and are distinct live
        // blocks.
        unsafe {
            ptr::copy_nonoverlapping(block, moved, layout.size().min(new_size));
            dealloc(block, layout);
        }
    }
    moved
}

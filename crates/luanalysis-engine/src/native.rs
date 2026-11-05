// Derived from Bun standalone/native.rs at 408c9374b266b45fe6b8260e09be73aa82a957cd.
// The unused JS parser macro-context symbol is omitted. Our OOM symbol is added
// at the end; the remaining standalone implementations are unchanged.
// Native symbols that Bun's C and C++ side provides to the parser, for binaries that link the parser and nothing
// else: this crate's command line tool and `bun_sema`'s tests. Plain loops instead of SIMD, and a bump allocator
// instead of mimalloc. Never part of the real build.

use core::ffi::{c_char, c_int, c_void};

// ───────────────────────────── mimalloc ─────────────────────────────

// With `--cfg bun_sema_mimalloc` the real mimalloc is linked instead, to measure with Bun's
// allocator.
#[cfg(not(bun_sema_mimalloc))]
mod fake_mimalloc {
    use super::*;

    #[repr(C)]
    struct Header {
        /// The pointer `malloc` returned, for a block that is freed individually. Otherwise the
        /// block before it in its chunk, tagged `BUMP`, and `FREED` once it is freed: a block freed
        /// at the end of its chunk gives its space back, with those freed just before it.
        base: *mut c_void,
        size: usize,
    }

    const BUMP: usize = 1;
    const FREED: usize = 2;

    const HEADER: usize = core::mem::size_of::<Header>();
    const CHUNK: usize = 1 << 20;

    struct Heap {
        chunks: Vec<*mut c_void>,
        at: usize,
        end: usize,
        /// The bytes that were handed out, with their headers and padding.
        used: usize,
        /// The header of the last block of the current chunk.
        last: *mut Header,
        /// The thread that created the heap: the only one that allocates from it.
        owner: usize,
        /// Once its owner retires, no thread-local pointer can still read this control block.
        owner_lifetime: std::sync::Weak<()>,
        #[cfg(test)]
        drop_probe: Option<std::sync::Arc<()>>,
    }

    const RETIRED_CURRENT: *mut Heap = core::ptr::without_provenance_mut(1);

    struct OwnerLifetime(std::sync::Arc<()>);

    impl Drop for OwnerLifetime {
        fn drop(&mut self) {
            // Other TLS destructors may still allocate or free. Disable the raw pointer before
            // the token expires, and do not let those later allocations reactivate it.
            CURRENT.with(|current| current.set(RETIRED_CURRENT));
        }
    }

    std::thread_local! {
        /// The heap this thread last allocated from. `mi_free` is told no heap.
        static CURRENT: core::cell::Cell<*mut Heap> = const { core::cell::Cell::new(core::ptr::null_mut()) };
        static OWNER_LIFETIME: OwnerLifetime = OwnerLifetime(std::sync::Arc::new(()));
    }

    fn this_thread() -> usize {
        CURRENT.with(|current| core::ptr::from_ref(current).addr())
    }

    /// `mi_heap_area_t`
    #[repr(C)]
    struct Area {
        blocks: *mut c_void,
        reserved: usize,
        committed: usize,
        used: usize,
        block_size: usize,
        full_block_size: usize,
        reserved1: *mut c_void,
    }

    /// `mi_block_visit_fun`
    type Visitor =
        extern "C" fn(*const c_void, *const Area, *mut c_void, usize, *mut c_void) -> bool;

    fn main_heap() -> *mut Heap {
        core::ptr::without_provenance_mut(8)
    }

    unsafe fn place(
        base: *mut c_void,
        from: usize,
        size: usize,
        align: usize,
        tag: *mut c_void,
    ) -> *mut c_void {
        let user = (from + HEADER).next_multiple_of(align.max(16));
        // SAFETY: the caller reserved `size + align + HEADER` bytes from `from`.
        unsafe {
            let user_ptr = base.byte_add(user - base.addr());
            user_ptr.cast::<Header>().sub(1).write(Header { base: tag, size });
            user_ptr
        }
    }

    fn global_alloc(size: usize, align: usize, zero: bool) -> *mut c_void {
        let total = size + align.max(16) + HEADER;
        // SAFETY: plain libc allocation.
        let base = unsafe {
            if zero {
                libc::calloc(1, total)
            } else {
                libc::malloc(total)
            }
        };
        if base.is_null() {
            return base;
        }
        // SAFETY: `total` bytes were reserved.
        unsafe { place(base, base.addr(), size, align, base) }
    }

    unsafe fn heap_alloc(heap: *mut Heap, size: usize, align: usize, zero: bool) -> *mut c_void {
        if heap == main_heap() {
            return global_alloc(size, align, zero);
        }
        CURRENT.with(|current| {
            if current.get() != RETIRED_CURRENT { current.set(heap); }
        });
        // SAFETY: a heap is used by the thread that created it.
        let heap = unsafe { &mut *heap };
        let total = size + align.max(16) + HEADER;
        heap.used += total;
        if heap.end - heap.at < total {
            let chunk = total.max(CHUNK);
            // SAFETY: plain libc allocation.
            let base = unsafe { libc::malloc(chunk) };
            if base.is_null() {
                return base;
            }
            heap.chunks.push(base);
            if total > CHUNK / 4 {
                // Allocated separately, so that the rest of the current chunk stays usable.
                // SAFETY: `total` bytes were reserved.
                let p = unsafe { place(base, base.addr(), size, align, core::ptr::without_provenance_mut(BUMP)) };
                if zero {
                    // SAFETY: `size` bytes at `p` are inside the chunk.
                    unsafe { core::ptr::write_bytes(p.cast::<u8>(), 0, size) };
                }
                return p;
            }
            heap.at = base.addr();
            heap.end = base.addr() + chunk;
            heap.last = core::ptr::null_mut();
        }
        let base = *heap.chunks.last().unwrap();
        let base = if heap.at >= base.addr() && heap.at < base.addr() + CHUNK {
            base
        } else {
            *heap
                .chunks
                .iter()
                .rev()
                .find(|c| heap.at >= c.addr() && heap.end <= c.addr() + CHUNK)
                .unwrap()
        };
        let tag = heap.last.map_addr(|addr| addr | BUMP).cast();
        // SAFETY: `total` bytes are left in the chunk.
        let p = unsafe { place(base, heap.at, size, align, tag) };
        heap.at = p.addr() + size;
        heap.last = p.cast::<Header>().wrapping_sub(1);
        if zero {
            // SAFETY: `size` bytes at `p` are inside the chunk.
            unsafe { core::ptr::write_bytes(p.cast::<u8>(), 0, size) };
        }
        p
    }

    /// A block's `base`, which another thread may be freeing it through.
    unsafe fn tag_of<'a>(block: *mut Header) -> &'a core::sync::atomic::AtomicPtr<c_void> {
        // SAFETY: `block` is a live header, and every access to `base` after `place` is atomic.
        unsafe { core::sync::atomic::AtomicPtr::from_ptr(&raw mut (*block).base) }
    }

    unsafe fn header(p: *const c_void) -> *const Header {
        // SAFETY: every returned block has a header immediately before it.
        unsafe { p.cast::<Header>().sub(1) }
    }

    unsafe fn release(p: *mut c_void) {
        if p.is_null() {
            return;
        }
        // SAFETY: see `header`.
        let block = unsafe { header(p) }.cast_mut();
        // SAFETY: see `header`.
        let base = unsafe { tag_of(block) };
        let tagged = base.load(core::sync::atomic::Ordering::Relaxed);
        if tagged.addr() & BUMP == 0 {
            // SAFETY: `tagged` came from `malloc`.
            unsafe { libc::free(tagged) };
            return;
        }
        // A block is freed once, by any thread. Its heap's own thread reads this below.
        base.store(tagged.map_addr(|addr| addr | FREED), core::sync::atomic::Ordering::Relaxed);
        let heap = CURRENT.with(core::cell::Cell::get);
        if heap.is_null() || heap == RETIRED_CURRENT {
            return;
        }
        // SAFETY: `CURRENT` is this thread's, and its heap outlives it (`mi_heap_destroy`).
        let heap = unsafe { &mut *heap };
        if heap.last != block {
            return;
        }
        // Blocks are allocated in order, and a chunk's are given back from its end.
        let mut top = block;
        while !top.is_null() {
            // SAFETY: `top` is a live header of this heap's current chunk.
            let tagged = unsafe { tag_of(top) }.load(core::sync::atomic::Ordering::Relaxed);
            if tagged.addr() & FREED == 0 {
                break;
            }
            heap.at = top.addr();
            top = tagged.map_addr(|addr| addr & !(BUMP | FREED)).cast();
        }
        heap.last = top;
    }

    #[unsafe(no_mangle)]
    extern "C" fn mi_heap_new() -> *mut c_void {
        Box::into_raw(Box::new(Heap {
            chunks: Vec::new(),
            at: 0,
            end: 0,
            used: 0,
            last: core::ptr::null_mut(),
            owner: this_thread(),
            // A later TLS destructor can create a heap after the owner token has retired.
            owner_lifetime: OWNER_LIFETIME.try_with(|owner| std::sync::Arc::downgrade(&owner.0)).unwrap_or_default(),
            #[cfg(test)]
            drop_probe: None,
        }))
        .cast()
    }
    #[unsafe(no_mangle)]
    unsafe extern "C" fn mi_heap_destroy(heap: *mut c_void) {
        // SAFETY: created by `mi_heap_new`.
        let mut heap = unsafe { Box::from_raw(heap.cast::<Heap>()) };
        for &chunk in &heap.chunks {
            // SAFETY: from `malloc`.
            unsafe { libc::free(chunk) };
        }
        heap.last = core::ptr::null_mut();
        heap.chunks = Vec::new();
        if heap.owner == this_thread() {
            CURRENT.with(|current| {
                if current.get() == &raw mut *heap {
                    current.set(core::ptr::null_mut());
                }
            });
        } else if heap.owner_lifetime.strong_count() != 0 {
            // Its thread may still name it as `CURRENT`, and finds nothing to give back.
            // Keep that control block alive. Joined workers have retired their token, so the
            // ordinary parallel session teardown drops theirs instead of leaking one per worker.
            Box::leak(heap);
        }
    }
    #[unsafe(no_mangle)]
    extern "C" fn mi_heap_main() -> *mut c_void {
        main_heap().cast()
    }
    /// The whole heap is one area of blocks of one byte. The blocks themselves are not visited.
    #[unsafe(no_mangle)]
    unsafe extern "C" fn mi_heap_visit_blocks(
        heap: *const c_void,
        _visit_all_blocks: bool,
        visitor: Option<Visitor>,
        arg: *mut c_void,
    ) -> bool {
        let Some(visitor) = visitor else {
            return false;
        };
        if heap == main_heap().cast_const().cast() {
            return true;
        }
        // SAFETY: created by `mi_heap_new`, and used by the thread that created it.
        let used = unsafe { (*heap.cast::<Heap>()).used };
        let area = Area {
            blocks: core::ptr::null_mut(),
            reserved: used,
            committed: used,
            used,
            block_size: 1,
            full_block_size: 1,
            reserved1: core::ptr::null_mut(),
        };
        visitor(heap, &raw const area, core::ptr::null_mut(), 1, arg)
    }
    #[unsafe(no_mangle)]
    extern "C" fn mi_collect(_force: bool) {}
    #[unsafe(no_mangle)]
    extern "C" fn mi_option_set(_option: u32, _value: core::ffi::c_long) {}
    #[unsafe(no_mangle)]
    unsafe extern "C" fn mi_heap_malloc(heap: *mut c_void, size: usize) -> *mut c_void {
        // SAFETY: `heap` comes from `mi_heap_new` or `mi_heap_main`, as mimalloc requires.
        unsafe { heap_alloc(heap.cast(), size, 16, false) }
    }
    #[unsafe(no_mangle)]
    unsafe extern "C" fn mi_heap_zalloc(heap: *mut c_void, size: usize) -> *mut c_void {
        // SAFETY: `heap` comes from `mi_heap_new` or `mi_heap_main`, as mimalloc requires.
        unsafe { heap_alloc(heap.cast(), size, 16, true) }
    }
    #[unsafe(no_mangle)]
    unsafe extern "C" fn mi_heap_malloc_aligned(
        heap: *mut c_void,
        size: usize,
        align: usize,
    ) -> *mut c_void {
        // SAFETY: `heap` comes from `mi_heap_new` or `mi_heap_main`, as mimalloc requires.
        unsafe { heap_alloc(heap.cast(), size, align, false) }
    }
    #[unsafe(no_mangle)]
    unsafe extern "C" fn mi_heap_zalloc_aligned(
        heap: *mut c_void,
        size: usize,
        align: usize,
    ) -> *mut c_void {
        // SAFETY: `heap` comes from `mi_heap_new` or `mi_heap_main`, as mimalloc requires.
        unsafe { heap_alloc(heap.cast(), size, align, true) }
    }
    #[unsafe(no_mangle)]
    unsafe extern "C" fn mi_heap_realloc_aligned(
        heap: *mut c_void,
        p: *mut c_void,
        size: usize,
        align: usize,
    ) -> *mut c_void {
        // SAFETY: `heap` comes from `mi_heap_new` or `mi_heap_main`, as mimalloc requires. `p` is
        // null or a block that one of these functions has returned, as mimalloc requires.
        unsafe {
            // The last block of a chunk grows or shrinks where it is.
            if !p.is_null() && heap.cast::<Heap>() != main_heap() {
                let bump = &mut *heap.cast::<Heap>();
                let block = header(p).cast_mut();
                if bump.last == block && p.addr().is_multiple_of(align.max(16)) && p.addr() + size <= bump.end {
                    (*block).size = size;
                    bump.at = p.addr() + size;
                    return p;
                }
            }
            let new = heap_alloc(heap.cast(), size, align, false);
            if !p.is_null() && !new.is_null() {
                core::ptr::copy_nonoverlapping(
                    p.cast::<u8>(),
                    new.cast::<u8>(),
                    (*header(p)).size.min(size),
                );
                release(p);
            }
            new
        }
    }
    #[unsafe(no_mangle)]
    extern "C" fn mi_malloc(size: usize) -> *mut c_void {
        global_alloc(size, 16, false)
    }
    #[unsafe(no_mangle)]
    extern "C" fn mi_zalloc(size: usize) -> *mut c_void {
        global_alloc(size, 16, true)
    }
    #[unsafe(no_mangle)]
    extern "C" fn mi_malloc_aligned(size: usize, align: usize) -> *mut c_void {
        global_alloc(size, align, false)
    }
    #[unsafe(no_mangle)]
    extern "C" fn mi_zalloc_aligned(size: usize, align: usize) -> *mut c_void {
        global_alloc(size, align, true)
    }
    #[unsafe(no_mangle)]
    unsafe extern "C" fn mi_realloc(p: *mut c_void, size: usize) -> *mut c_void {
        // SAFETY: `p` is null or a block that one of these functions has returned, as mimalloc
        // requires.
        unsafe { mi_heap_realloc_aligned(main_heap().cast(), p, size, 16) }
    }
    #[unsafe(no_mangle)]
    unsafe extern "C" fn mi_realloc_aligned(
        p: *mut c_void,
        size: usize,
        align: usize,
    ) -> *mut c_void {
        // SAFETY: `p` is null or a block that one of these functions has returned, as mimalloc
        // requires.
        unsafe { mi_heap_realloc_aligned(main_heap().cast(), p, size, align) }
    }
    #[unsafe(no_mangle)]
    unsafe extern "C" fn mi_free(p: *mut c_void) {
        // SAFETY: `p` is null or a block that one of these functions has returned, as mimalloc
        // requires.
        unsafe { release(p) }
    }
    #[unsafe(no_mangle)]
    unsafe extern "C" fn mi_free_size(p: *mut c_void, _size: usize) {
        // SAFETY: `p` is null or a block that one of these functions has returned, as mimalloc
        // requires.
        unsafe { release(p) }
    }
    #[unsafe(no_mangle)]
    unsafe extern "C" fn mi_free_size_aligned(p: *mut c_void, _size: usize, _align: usize) {
        // SAFETY: `p` is null or a block that one of these functions has returned, as mimalloc
        // requires.
        unsafe { release(p) }
    }
    #[unsafe(no_mangle)]
    extern "C" fn mi_expand(_p: *mut c_void, _size: usize) -> *mut c_void {
        core::ptr::null_mut()
    }
    #[unsafe(no_mangle)]
    unsafe extern "C" fn mi_malloc_usable_size(p: *const c_void) -> usize {
        if p.is_null() {
            0
        } else {
            // SAFETY: `p` is a block that one of these functions has returned. See `header`.
            unsafe { (*header(p)).size }
        }
    }
    #[unsafe(no_mangle)]
    unsafe extern "C" fn mi_usable_size(p: *const c_void) -> usize {
        // SAFETY: `p` is null or a block that one of these functions has returned, as mimalloc
        // requires.
        unsafe { mi_malloc_usable_size(p) }
    }
    #[unsafe(no_mangle)]
    extern "C" fn mi_is_in_heap_region(_p: *const c_void) -> bool {
        true
    }
    #[unsafe(no_mangle)]
    unsafe extern "C" fn mi_process_info(
        a: *mut usize,
        b: *mut usize,
        c: *mut usize,
        d: *mut usize,
        e: *mut usize,
        f: *mut usize,
        g: *mut usize,
        h: *mut usize,
    ) {
        for p in [a, b, c, d, e, f, g, h] {
            if !p.is_null() {
                // SAFETY: it is not null, so the caller wants a number there.
                unsafe { *p = 0 };
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::sync::{Arc, Weak, mpsc};

        fn tracked_heap() -> (*mut c_void, Weak<()>) {
            let heap = mi_heap_new();
            let probe = Arc::new(());
            let dropped = Arc::downgrade(&probe);
            // SAFETY: this test owns the newly allocated heap before publishing it.
            unsafe { (*heap.cast::<Heap>()).drop_probe = Some(probe); }
            (heap, dropped)
        }

        #[test]
        fn joined_worker_heap_control_is_released_with_its_arena() {
            let (address, dropped) = std::thread::spawn(|| {
                let (heap, dropped) = tracked_heap();
                // SAFETY: this thread owns the live heap.
                unsafe { mi_heap_zalloc(heap, 64); }
                (heap as usize, dropped)
            }).join().unwrap();
            assert!(dropped.upgrade().is_some());
            // SAFETY: the owner has joined and transferred its live heap for destruction.
            unsafe { mi_heap_destroy(address as *mut c_void); }
            assert!(dropped.upgrade().is_none(), "the heap's control allocation must be dropped too");
        }

        #[test]
        fn live_foreign_destruction_keeps_the_current_pointer_valid() {
            let (send, receive) = mpsc::channel();
            let (resume, resumed) = mpsc::channel();
            let owner = std::thread::spawn(move || {
                let other = mi_heap_new();
                // SAFETY: this thread owns both heaps, and the first block stays live.
                let block = unsafe { mi_heap_malloc(other, 64) };
                let (heap, dropped) = tracked_heap();
                unsafe { mi_heap_malloc(heap, 64); }
                send.send((heap as usize, dropped)).unwrap();
                resumed.recv().unwrap();
                assert_eq!(CURRENT.with(core::cell::Cell::get), heap.cast::<Heap>());
                // SAFETY: `block` belongs to the other, still-live heap. Its release may inspect
                // CURRENT's control block even though that heap's chunks were freed remotely.
                unsafe { mi_free(block); mi_heap_destroy(other); }
            });
            let (address, dropped) = receive.recv().unwrap();
            // SAFETY: the owner is waiting and has transferred the second heap for destruction.
            unsafe { mi_heap_destroy(address as *mut c_void); }
            assert!(dropped.upgrade().is_some(), "a live owner's CURRENT still needs its control block");
            resume.send(()).unwrap();
            owner.join().unwrap();
            // SAFETY: this is the intentionally retained live-foreign tombstone, with no chunks;
            // the owner has now joined, so the test may reclaim it without another heap destroy.
            unsafe { drop(Box::from_raw(address as *mut Heap)); }
            assert!(dropped.upgrade().is_none());
        }

        struct LateCleanup(std::cell::RefCell<Option<mpsc::Sender<(bool, bool, bool)>>>);

        impl Drop for LateCleanup {
            fn drop(&mut self) {
                let before = CURRENT.with(core::cell::Cell::get) == RETIRED_CURRENT;
                let (heap, dropped) = tracked_heap();
                // SAFETY: this late TLS destructor owns the newly created heap and its block.
                unsafe {
                    let block = mi_heap_malloc(heap, 64);
                    mi_free(block);
                    mi_heap_destroy(heap);
                }
                let after = CURRENT.with(core::cell::Cell::get) == RETIRED_CURRENT;
                self.0.get_mut().take().unwrap().send((before, after, dropped.upgrade().is_none())).unwrap();
            }
        }

        std::thread_local! {
            static LATE_CLEANUP: LateCleanup = const { LateCleanup(std::cell::RefCell::new(None)) };
        }

        #[test]
        fn later_tls_destructors_do_not_reactivate_the_current_heap() {
            let (send, receive) = mpsc::channel();
            std::thread::spawn(move || {
                // Initialized first, so its destructor runs after OWNER_LIFETIME's.
                LATE_CLEANUP.with(|cleanup| *cleanup.0.borrow_mut() = Some(send));
                let heap = mi_heap_new();
                // SAFETY: this thread owns the newly created heap.
                unsafe { mi_heap_destroy(heap); }
            }).join().unwrap();
            assert_eq!(receive.recv().unwrap(), (true, true, true));
        }
    }
}

// ───────────────────────────── highway ─────────────────────────────

unsafe fn bytes<'a>(p: *const u8, len: usize) -> &'a [u8] {
    if len == 0 {
        &[]
    } else {
        // SAFETY: the contract of this function: `p` and `len` are those of a slice that lives for
        // `'a`.
        unsafe { core::slice::from_raw_parts(p, len) }
    }
}

/// Tests 32 bytes at a time without a branch in between, which the compiler turns into vector instructions: what is measured
/// with these stand-ins should not be far from what the kernels do.
fn first(text: &[u8], f: impl Fn(u8) -> bool) -> usize {
    let mut at = 0;
    for chunk in text.as_chunks::<32>().0 {
        if chunk.iter().fold(false, |is_found, &c| is_found | f(c)) {
            break;
        }
        at += 32;
    }
    at + text[at..]
        .iter()
        .position(|&c| f(c))
        .unwrap_or(text.len() - at)
}

// Weak: where the kernels themselves are linked, they count.
#[unsafe(no_mangle)]
#[linkage = "weak"]
unsafe extern "C" fn highway_index_of_char(p: *const u8, len: usize, needle: u8) -> usize {
    // SAFETY: the caller passes a slice, as a pointer and a length.
    first(unsafe { bytes(p, len) }, |c| c == needle)
}
#[unsafe(no_mangle)]
#[linkage = "weak"]
unsafe extern "C" fn highway_last_index_of_char(p: *const u8, len: usize, needle: u8) -> usize {
    // SAFETY: the caller passes a slice, as a pointer and a length.
    unsafe { bytes(p, len) }
        .iter()
        .rposition(|&c| c == needle)
        .unwrap_or(len)
}
#[unsafe(no_mangle)]
#[linkage = "weak"]
unsafe extern "C" fn highway_count_char(p: *const u8, len: usize, needle: u8) -> usize {
    // SAFETY: the caller passes a slice, as a pointer and a length.
    let text = unsafe { bytes(p, len) };
    // Sums that fit in a byte, which the compiler adds up with vector instructions.
    text.chunks(192)
        .map(|chunk| usize::from(chunk.iter().map(|&c| u8::from(c == needle)).sum::<u8>()))
        .sum()
}
#[unsafe(no_mangle)]
#[linkage = "weak"]
unsafe extern "C" fn highway_line_starts(p: *const u8, len: usize, out: *mut u32) -> usize {
    // SAFETY: the caller passes a slice, as a pointer and a length.
    let text = unsafe { bytes(p, len) };
    let mut written = 0;
    for (i, _) in text.iter().enumerate().filter(|it| *it.1 == b'\n') {
        // SAFETY: `out` has room for one number for every line feed of the text.
        unsafe { out.add(written).write(i as u32 + 1) };
        written += 1;
    }
    written
}
#[unsafe(no_mangle)]
#[linkage = "weak"]
unsafe extern "C" fn highway_odd_blocks(p: *const u8, len: usize, out: *mut u64) {
    // SAFETY: the caller passes a slice, as a pointer and a length.
    let text = unsafe { bytes(p, len) };
    for (word, blocks) in text.chunks(64 * 64).enumerate() {
        let is_odd = |c: &u8| !matches!(c, 0x20..=0x5B | 0x5D..=0x7E | b'\t' | b'\n' | b'\r');
        let marks = blocks.chunks(64).enumerate();
        let marks = marks.fold(0u64, |marks, (i, block)| {
            marks | u64::from(block.iter().any(is_odd)) << i
        });
        // SAFETY: `out` has a word for every 64 blocks of 64 bytes, whole or not.
        unsafe { out.add(word).write(marks) };
    }
}
#[unsafe(no_mangle)]
#[linkage = "weak"]
unsafe extern "C" fn highway_scanner_run_stops(
    p: *const u8,
    len: usize,
    out: *mut u8,
    plane_len: usize,
) {
    // SAFETY: the caller passes a slice, as a pointer and a length.
    let text = unsafe { bytes(p, len) };
    // SAFETY: `out` has three bitmaps, `plane_len` bytes apart, of 8 bytes for every 64 of the text.
    let planes =
        unsafe { core::slice::from_raw_parts_mut(out, 2 * plane_len + len.div_ceil(64) * 8) };
    for (block, chunk) in text.chunks(64).enumerate() {
        let mut stops = [u64::MAX; 3];
        for (i, &c) in chunk.iter().enumerate() {
            let is_name = c.is_ascii_alphanumeric() || c == b'_' || c == b'$';
            let is_blank = c == b'\t' || c == b' ';
            let is_plain =
                (0x20..0x80).contains(&c) && !matches!(c, b'"' | b'\'' | b'`' | b'$' | b'\\');
            for (plane, is_in_run) in stops.iter_mut().zip([is_name, is_blank, is_plain]) {
                *plane &= !(u64::from(is_in_run) << i);
            }
        }
        for (plane, stops) in stops.iter().enumerate() {
            let at = plane * plane_len + block * 8;
            planes[at..at + 8].copy_from_slice(&stops.to_le_bytes());
        }
    }
}
#[unsafe(no_mangle)]
#[linkage = "weak"]
unsafe extern "C" fn highway_index_of_any_char(
    p: *const u8,
    len: usize,
    chars: *const u8,
    chars_len: usize,
) -> usize {
    // SAFETY: the caller passes a slice, as a pointer and a length.
    let chars = unsafe { bytes(chars, chars_len) };
    // SAFETY: the caller passes a slice, as a pointer and a length.
    let text = unsafe { bytes(p, len) };
    match *chars {
        [a] => first(text, |c| c == a),
        // One character after the other for each 32 bytes: that is what the compiler makes good vector code of.
        _ if chars.len() <= 8 => {
            let has = |chunk: &[u8], wanted: u8| {
                chunk
                    .iter()
                    .fold(false, |is_found, &c| is_found | (c == wanted))
            };
            let whole = text
                .as_chunks::<32>()
                .0
                .iter()
                .take_while(|chunk| !chars.iter().any(|&wanted| has(&chunk[..], wanted)))
                .count();
            let at = whole * 32;
            at + text[at..]
                .iter()
                .position(|c| chars.contains(c))
                .unwrap_or(len - at)
        }
        _ => {
            let mut is_in_set = [false; 256];
            chars.iter().for_each(|&c| is_in_set[c as usize] = true);
            text.iter()
                .position(|&c| is_in_set[c as usize])
                .unwrap_or(len)
        }
    }
}
#[unsafe(no_mangle)]
#[linkage = "weak"]
unsafe extern "C" fn highway_last_index_of_any_char(
    p: *const u8,
    len: usize,
    chars: *const u8,
    chars_len: usize,
) -> usize {
    // SAFETY: the caller passes a slice, as a pointer and a length.
    let chars = unsafe { bytes(chars, chars_len) };
    // SAFETY: the caller passes a slice, as a pointer and a length.
    unsafe { bytes(p, len) }
        .iter()
        .rposition(|c| chars.contains(c))
        .unwrap_or(len)
}
#[unsafe(no_mangle)]
#[linkage = "weak"]
unsafe extern "C" fn highway_memmem(
    h: *const u8,
    h_len: usize,
    n: *const u8,
    n_len: usize,
) -> *const u8 {
    // SAFETY: the caller passes two slices, each as a pointer and a length.
    let (hay, needle) = unsafe { (bytes(h, h_len), bytes(n, n_len)) };
    if needle.is_empty() {
        return h;
    }
    if hay.len() < needle.len() {
        return core::ptr::null();
    }
    // As the kernel does: where the first and the last byte of the needle are both in place, 32 positions at a time.
    let last = hay.len() - needle.len();
    let (head, tail) = (needle[0], needle[needle.len() - 1]);
    let (heads, tails) = (&hay[..=last], &hay[needle.len() - 1..]);
    let mut at = 0;
    while at <= last {
        for (heads, tails) in heads[at..]
            .as_chunks::<32>()
            .0
            .iter()
            .zip(tails[at..].as_chunks::<32>().0)
        {
            if heads.iter().zip(tails).fold(false, |is_found, (&a, &b)| {
                is_found | ((a == head) & (b == tail))
            }) {
                break;
            }
            at += 32;
        }
        let Some(found) = heads[at..]
            .iter()
            .zip(&tails[at..])
            .position(|(&a, &b)| a == head && b == tail)
        else {
            break;
        };
        at += found;
        if hay[at..at + needle.len()] == *needle {
            // SAFETY: `at` is a position in `hay`, which begins at `h`.
            return unsafe { h.add(at) };
        }
        at += 1;
    }
    core::ptr::null()
}
#[unsafe(no_mangle)]
#[linkage = "weak"]
unsafe extern "C" fn highway_memrmem(
    h: *const u8,
    h_len: usize,
    n: *const u8,
    n_len: usize,
) -> usize {
    // SAFETY: the caller passes two slices, each as a pointer and a length.
    let (hay, needle) = unsafe { (bytes(h, h_len), bytes(n, n_len)) };
    if needle.is_empty() {
        return h_len;
    }
    if hay.len() < needle.len() {
        return usize::MAX;
    }
    (0..=hay.len() - needle.len())
        .rev()
        .find(|&i| hay[i..i + needle.len()] == *needle)
        .unwrap_or(usize::MAX)
}
#[unsafe(no_mangle)]
#[linkage = "weak"]
unsafe extern "C" fn highway_memrmem16(
    h: *const u16,
    h_len: usize,
    n: *const u16,
    n_len: usize,
) -> usize {
    if n_len == 0 {
        return h_len;
    }
    if h_len < n_len {
        return usize::MAX;
    }
    // SAFETY: the caller passes two slices, each as a pointer and a length.
    let (hay, needle) = unsafe {
        (
            core::slice::from_raw_parts(h, h_len),
            core::slice::from_raw_parts(n, n_len),
        )
    };
    (0..=hay.len() - needle.len())
        .rev()
        .find(|&i| hay[i..i + needle.len()] == *needle)
        .unwrap_or(usize::MAX)
}
#[unsafe(no_mangle)]
#[linkage = "weak"]
unsafe extern "C" fn highway_index_of_interesting_character_in_string_literal(
    p: *const u8,
    len: usize,
    quote: u8,
) -> usize {
    // SAFETY: the caller passes a slice, as a pointer and a length.
    first(unsafe { bytes(p, len) }, |c| {
        c == quote || c == b'\\' || !(0x20..=0x7E).contains(&c)
    })
}
#[unsafe(no_mangle)]
#[linkage = "weak"]
unsafe extern "C" fn highway_index_of_interesting_character_in_multiline_comment(
    p: *const u8,
    len: usize,
) -> usize {
    // SAFETY: the caller passes a slice, as a pointer and a length.
    first(unsafe { bytes(p, len) }, |c| {
        c == b'*' || c == b'\r' || c == b'\n' || c > 127
    })
}
#[unsafe(no_mangle)]
#[linkage = "weak"]
unsafe extern "C" fn highway_index_of_newline_or_non_ascii_or_hash_or_at(
    p: *const u8,
    len: usize,
) -> usize {
    // SAFETY: the caller passes a slice, as a pointer and a length.
    first(unsafe { bytes(p, len) }, |c| {
        c == b'#' || c == b'@' || !(0x20..=0x7E).contains(&c)
    })
}
#[unsafe(no_mangle)]
#[linkage = "weak"]
unsafe extern "C" fn highway_index_of_space_or_newline_or_non_ascii(
    p: *const u8,
    len: usize,
) -> usize {
    // SAFETY: the caller passes a slice, as a pointer and a length.
    first(unsafe { bytes(p, len) }, |c| c <= b' ' || c > 127)
}
#[unsafe(no_mangle)]
#[linkage = "weak"]
unsafe extern "C" fn highway_contains_newline_or_non_ascii_or_quote(
    p: *const u8,
    len: usize,
) -> bool {
    // SAFETY: the caller passes a slice, as a pointer and a length.
    unsafe { bytes(p, len) }
        .iter()
        .any(|&c| !(0x20..=127).contains(&c) || c == b'"')
}
#[unsafe(no_mangle)]
#[linkage = "weak"]
unsafe extern "C" fn highway_index_of_needs_escape_for_javascript_string(
    p: *const u8,
    len: usize,
    quote: u8,
) -> usize {
    // SAFETY: the caller passes a slice, as a pointer and a length.
    first(unsafe { bytes(p, len) }, |c| {
        c == quote || c == b'\\' || !(0x20..=0x7E).contains(&c) || (quote == b'`' && c == b'$')
    })
}
/// `BUN_JSON_IDX_ODDITY`: `StructuralIndex` goes on with its scalar indexer.
#[unsafe(no_mangle)]
#[linkage = "weak"]
unsafe extern "C" fn highway_json_index_chunk(
    _input: *const u8,
    _len: usize,
    _base_offset: usize,
    _out_indices: *mut u32,
    _out_dirty: *mut u64,
    _inout_state: *mut u64,
    out_flags: *mut u32,
) -> usize {
    // SAFETY: the caller passes a place for the flags.
    unsafe { *out_flags = 1 << 3 };
    0
}

// ───────────────────────────── simdutf ─────────────────────────────

#[repr(C)]
struct SimdutfResult {
    status: c_int,
    count: usize,
}

#[unsafe(no_mangle)]
unsafe extern "C" fn simdutf__validate_utf8(p: *const u8, len: usize) -> bool {
    // SAFETY: the caller passes a slice, as a pointer and a length.
    core::str::from_utf8(unsafe { bytes(p, len) }).is_ok()
}
#[unsafe(no_mangle)]
unsafe extern "C" fn simdutf__validate_utf8_with_errors(p: *const u8, len: usize) -> SimdutfResult {
    // SAFETY: the caller passes a slice, as a pointer and a length.
    match core::str::from_utf8(unsafe { bytes(p, len) }) {
        Ok(_) => SimdutfResult {
            status: 0,
            count: len,
        },
        // simdutf's HEADER_BITS
        Err(error) => SimdutfResult {
            status: 1,
            count: error.valid_up_to(),
        },
    }
}
#[unsafe(no_mangle)]
unsafe extern "C" fn simdutf__convert_utf8_to_utf16le_with_errors(
    p: *const u8,
    len: usize,
    out: *mut u16,
) -> SimdutfResult {
    // SAFETY: the caller passes a slice, as a pointer and a length.
    let text = match core::str::from_utf8(unsafe { bytes(p, len) }) {
        Ok(text) => text,
        // simdutf's HEADER_BITS
        Err(error) => {
            return SimdutfResult {
                status: 1,
                count: error.valid_up_to(),
            };
        }
    };
    let mut written = 0;
    for unit in text.encode_utf16() {
        // SAFETY: the caller has reserved room for the longest possible result, as simdutf
        // requires.
        unsafe { out.add(written).write(unit) };
        written += 1;
    }
    SimdutfResult {
        status: 0,
        count: written,
    }
}
#[unsafe(no_mangle)]
unsafe extern "C" fn simdutf__validate_ascii(p: *const u8, len: usize) -> bool {
    // SAFETY: the caller passes a slice, as a pointer and a length.
    unsafe { bytes(p, len) }.is_ascii()
}
#[unsafe(no_mangle)]
unsafe extern "C" fn simdutf__validate_ascii_with_errors(
    p: *const u8,
    len: usize,
) -> SimdutfResult {
    // SAFETY: the caller passes a slice, as a pointer and a length.
    match Some(first(unsafe { bytes(p, len) }, |c| c > 127)).filter(|&at| at < len) {
        // simdutf's TOO_LARGE
        Some(at) => SimdutfResult {
            status: 5,
            count: at,
        },
        None => SimdutfResult {
            status: 0,
            count: len,
        },
    }
}
unsafe fn utf16_units<'a>(p: *const u16, len: usize) -> &'a [u16] {
    if len == 0 {
        &[]
    } else {
        // SAFETY: the contract of this function: `p` and `len` are those of a slice that lives for
        // `'a`.
        unsafe { core::slice::from_raw_parts(p, len) }
    }
}
#[unsafe(no_mangle)]
unsafe extern "C" fn simdutf__utf8_length_from_utf16le(p: *const u16, len: usize) -> usize {
    // SAFETY: the caller passes a slice, as a pointer and a length.
    let units = unsafe { utf16_units(p, len) };
    char::decode_utf16(units.iter().copied())
        .map(|c| c.map_or(3, char::len_utf8))
        .sum()
}
#[unsafe(no_mangle)]
unsafe extern "C" fn simdutf__utf8_length_from_utf16le_with_replacement(
    p: *const u16,
    len: usize,
) -> usize {
    // SAFETY: the caller passes a slice, as a pointer and a length.
    let units = unsafe { utf16_units(p, len) };
    char::decode_utf16(units.iter().copied())
        .map(|c| c.map_or(3, char::len_utf8))
        .sum()
}
#[unsafe(no_mangle)]
unsafe extern "C" fn simdutf__utf16_length_from_utf8(p: *const u8, len: usize) -> usize {
    // SAFETY: the caller passes a slice, as a pointer and a length.
    unsafe { bytes(p, len) }
        .iter()
        .map(|&c| usize::from((c & 0xC0) != 0x80) + usize::from(c >= 0xF0))
        .sum()
}
#[unsafe(no_mangle)]
unsafe extern "C" fn simdutf__convert_utf16le_to_utf8_with_errors(
    p: *const u16,
    len: usize,
    out: *mut u8,
) -> SimdutfResult {
    let mut written = 0;
    let mut read = 0;
    // SAFETY: the caller passes a slice, as a pointer and a length.
    let units = unsafe { utf16_units(p, len) };
    for c in char::decode_utf16(units.iter().copied()) {
        let Ok(c) = c else {
            // simdutf's SURROGATE
            return SimdutfResult {
                status: 6,
                count: read,
            };
        };
        let mut buffer = [0u8; 4];
        let encoded = c.encode_utf8(&mut buffer);
        // SAFETY: the caller has reserved room for the longest possible result, as simdutf
        // requires.
        unsafe {
            core::ptr::copy_nonoverlapping(encoded.as_ptr(), out.add(written), encoded.len())
        };
        written += encoded.len();
        read += c.len_utf16();
    }
    SimdutfResult {
        status: 0,
        count: written,
    }
}
#[unsafe(no_mangle)]
unsafe extern "C" fn simdutf__base64_encode(
    _p: *const u8,
    _len: usize,
    _out: *mut u8,
    _url: c_int,
) -> usize {
    unreachable!("nothing that parses encodes base64")
}

// ───────────────────────────── widths ─────────────────────────────

/// An approximation. Bun's implementation is grapheme-aware.
fn width_of(c: char) -> usize {
    match c as u32 {
        0x0300..=0x036F | 0x200B..=0x200F | 0xFE00..=0xFE0F => 0,
        0x1100..=0x115F
        | 0x2E80..=0xA4CF
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE30..=0xFE6F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1F300..=0x1FAFF
        | 0x20000..=0x3FFFD => 2,
        _ => 1,
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn Bun__visibleWidthExcludeANSI_utf8(p: *const u8, len: usize) -> usize {
    // SAFETY: the caller passes a slice.
    let bytes = unsafe { core::slice::from_raw_parts(p, len) };
    bstr::ByteSlice::chars(bytes).map(width_of).sum()
}

#[unsafe(no_mangle)]
unsafe extern "C" fn Bun__visibleWidthExcludeANSI_utf8IndexAtWidth(
    p: *const u8,
    len: usize,
    max_width: usize,
) -> usize {
    // SAFETY: the caller passes a slice.
    let bytes = unsafe { core::slice::from_raw_parts(p, len) };
    let Ok(text) = core::str::from_utf8(bytes) else {
        return len.min(max_width);
    };
    let mut width = 0;
    for (at, c) in text.char_indices() {
        width += width_of(c);
        if width > max_width {
            return at;
        }
    }
    len
}

// ───────────────────────────── the rest ─────────────────────────────

#[unsafe(no_mangle)]
extern "C" fn Bun__StackCheck__getMaxStack() -> *mut c_void {
    STACK_LIMIT.with(|limit| core::ptr::without_provenance_mut(limit.get()))
}

thread_local! {
    static STACK_LIMIT: core::cell::Cell<usize> = const { core::cell::Cell::new(0) };
}

/// Called by a thread of the pool when it starts, near the top of its stack.
#[unsafe(no_mangle)]
extern "C" fn Bun__StackCheck__initialize() {
    set_stack_size(crate::CHECKER_STACK_BUDGET);
}
#[unsafe(no_mangle)]
extern "C" fn WTF__numberOfProcessorCores() -> c_int {
    std::thread::available_parallelism().map_or(4, |n| n.get() as c_int)
}
#[cfg(not(bun_sema_mimalloc))]
#[unsafe(no_mangle)]
extern "C" fn mi_thread_set_in_threadpool() {}
#[cfg(not(bun_sema_mimalloc))]
#[unsafe(no_mangle)]
extern "C" fn mi_on_thread_idle() {}
#[cfg(not(bun_sema_mimalloc))]
#[unsafe(no_mangle)]
extern "C" fn mi_on_thread_idle_start() -> bool {
    false
}
#[cfg(not(bun_sema_mimalloc))]
#[unsafe(no_mangle)]
extern "C" fn mi_on_thread_idle_end() {}

/// Sets the approximate remaining stack of the current thread. Without it recursion is unbounded.
pub fn set_stack_size(remaining: usize) {
    let probe = 0u8;
    STACK_LIMIT.with(|limit| limit.set((&raw const probe).addr().saturating_sub(remaining)));
}

#[unsafe(no_mangle)]
extern "C" fn bun_cpu_features() -> u8 {
    0
}
#[unsafe(no_mangle)]
extern "C" fn bun_restore_stdio() {}
#[unsafe(no_mangle)]
extern "C" fn on_before_reload_process_posix() {}
#[unsafe(no_mangle)]
extern "C" fn getRSS(rss: &mut usize) -> c_int {
    *rss = 0;
    -1
}
#[unsafe(no_mangle)]
extern "C" fn getPeakRSS(peak: &mut usize) -> c_int {
    *peak = 0;
    -1
}
#[unsafe(no_mangle)]
extern "C" fn is_executable_file(_path: *const c_char) -> bool {
    false
}
#[unsafe(no_mangle)]
extern "C" fn compress2(
    _d: *mut u8,
    _dl: *mut usize,
    _s: *const u8,
    _sl: usize,
    _level: c_int,
) -> c_int {
    -2
}
/// The digits of the mantissa of `text`, from the first that is not 0.
fn mantissa_digits(text: &[u8]) -> impl Iterator<Item = u8> {
    let end = bun_core::strings::index_of_char_usize(text, b'e').unwrap_or(text.len());
    let digits = text[..end].iter().copied().filter(u8::is_ascii_digit);
    digits.skip_while(|&digit| digit == b'0')
}

/// ECMA-262 `Number::toString`: of two shortest forms that are equally close to `number`, the one
/// whose last digit is even. `text`: `number` as Rust writes it, which takes the upper one.
fn round_tie_to_even(text: &mut [u8], number: f64) {
    use std::io::Write;
    let end = bun_core::strings::index_of_char_usize(text, b'e').unwrap_or(text.len());
    let Some(last) = end.checked_sub(1) else {
        return;
    };
    if !text[last].is_ascii_digit() || text[last].is_multiple_of(2) {
        return;
    }
    text[last] -= 1;
    let written = core::str::from_utf8(text).ok();
    let lower: Option<f64> = written.and_then(|s| s.parse().ok());
    // In a tie the exact value, which has at most 767 digits, is the lower form and a 5.
    let mut exact: Vec<u8> = Vec::new();
    if lower == Some(number) {
        let _ = write!(exact, "{number:.766e}");
    }
    let is_tie = {
        let mut rest = mantissa_digits(&exact);
        let mut halfway = mantissa_digits(text).chain(*b"5");
        halfway.all(|digit| rest.next() == Some(digit)) && rest.all(|digit| digit == b'0')
    };
    if !is_tie {
        text[last] += 1;
    }
}

/// `String(number)`, as `WTF::numberToString` writes it.
#[unsafe(no_mangle)]
extern "C" fn WTF__dtoa(buf: &mut [u8; 124], number: f64) -> usize {
    use std::io::Write;
    let mut text = Vec::new();
    if number.is_nan() {
        text.extend_from_slice(b"NaN");
    } else if number.is_infinite() {
        text.extend_from_slice(if number > 0.0 {
            b"Infinity"
        } else {
            b"-Infinity"
        });
    } else if number == 0.0 {
        text.push(b'0');
    } else if number.abs() >= 1e21 || number.abs() < 1e-6 {
        let _ = write!(text, "{number:e}");
        round_tie_to_even(&mut text, number);
        if let Some(e) = bun_core::strings::index_of_char_usize(&text, b'e')
            && text.get(e + 1) != Some(&b'-')
        {
            text.insert(e + 1, b'+');
        }
    } else {
        let _ = write!(text, "{number}");
        round_tie_to_even(&mut text, number);
    }
    buf[..text.len()].copy_from_slice(&text);
    text.len()
}

/// The longest prefix of `p` that is a decimal number, as `WTF::parseDouble` parses it.
#[unsafe(no_mangle)]
unsafe extern "C" fn WTF__parseDouble(p: *const u8, len: usize, counted: *mut usize) -> f64 {
    // SAFETY: the caller passes a slice, as a pointer and a length.
    let text = unsafe { bytes(p, len) };
    let mut end = 0;
    let digits = |from: usize| {
        from + text[from..]
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .count()
    };
    if matches!(text.first(), Some(b'+' | b'-')) {
        end = 1;
    }
    let int_end = digits(end);
    let mut has_digits = int_end > end;
    end = int_end;
    if text.get(end) == Some(&b'.') {
        let fraction_end = digits(end + 1);
        if fraction_end > end + 1 || has_digits {
            has_digits |= fraction_end > end + 1;
            end = fraction_end;
        }
    }
    if !has_digits {
        // SAFETY: the caller passes a place for the count.
        unsafe { *counted = 0 };
        return 0.0;
    }
    if matches!(text.get(end), Some(b'e' | b'E')) {
        let mut at = end + 1;
        if matches!(text.get(at), Some(b'+' | b'-')) {
            at += 1;
        }
        let exponent_end = digits(at.min(text.len()));
        if exponent_end > at {
            end = exponent_end;
        }
    }
    // SAFETY: the caller passes a place for the count.
    unsafe { *counted = end };
    core::str::from_utf8(&text[..end])
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0)
}

#[unsafe(no_mangle)]
extern "Rust" fn __bun_crash_handler_out_of_memory() -> ! {
    std::process::abort()
}

// ───────────────────────────── Windows ─────────────────────────────

// The MSVC linker resolves every symbol an included object references before it
// discards unused functions, unlike the Unix linkers, so Bun's Windows code needs
// definitions even where the checker never calls it. Process setup is a no-op;
// the rest is unreachable from checking and aborts if reached.
#[cfg(windows)]
#[allow(non_snake_case, non_upper_case_globals)]
mod windows {
    use core::ffi::c_int;

    #[unsafe(no_mangle)]
    extern "C" fn clock_gettime_monotonic(sec: &mut i64, nsec: &mut i64) {
        static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        let elapsed = START.get_or_init(std::time::Instant::now).elapsed();
        *sec = elapsed.as_secs() as i64;
        *nsec = i64::from(elapsed.subsec_nanos());
    }
    #[unsafe(no_mangle)]
    extern "C" fn getpid() -> c_int { std::process::id() as c_int }
    #[unsafe(no_mangle)]
    extern "C" fn Bun__ramSize() -> usize { 16 << 30 }
    // Whether each standard stream was closed at startup; none is.
    #[unsafe(no_mangle)]
    static bun_is_stdio_null: [core::sync::atomic::AtomicI32; 3] =
        [core::sync::atomic::AtomicI32::new(0), core::sync::atomic::AtomicI32::new(0), core::sync::atomic::AtomicI32::new(0)];
    #[unsafe(no_mangle)]
    extern "C" fn bun_initialize_process() {}
    #[unsafe(no_mangle)]
    extern "C" fn Bun__lockThreadSuspensionForExit() {}
    #[unsafe(no_mangle)]
    extern "C" fn windows_enable_stdio_inheritance() {}
    #[unsafe(no_mangle)]
    extern "C" fn uv_disable_stdio_inheritance() {}

    macro_rules! unreachable_symbols {
        ($($name:ident),* $(,)?) => {
            $(#[unsafe(no_mangle)] extern "C" fn $name() -> ! { std::process::abort() })*
        };
    }
    unreachable_symbols!(
        BunString__createAtom,
        BunString__createExternalGloballyAllocatedLatin1,
        BunString__createExternalGloballyAllocatedUTF16,
        BunString__createStaticExternal,
        BunString__createStaticExternalLatin1WithHash,
        BunString__createStaticExternalUTF16WithHash,
        BunString__fromBytes,
        BunString__fromLatin1,
        BunString__fromLatin1Unitialized,
        BunString__fromUTF16,
        BunString__fromUTF16ToLatin1,
        BunString__fromUTF16Unitialized,
        BunString__makeThreadShareable,
        BunString__threadIsolatedCopy,
        BunString__tryCreateAtom,
        Bun__ANSI__next,
        Bun__WTFStringImpl__destroy,
        Bun__WTFStringImpl__ensureHash,
        Bun__collateUTF16,
        Bun__collateUTF8,
        Bun__visibleWidthExcludeANSI_latin1,
        Bun__visibleWidthExcludeANSI_utf16,
        WTFStringImpl__isThreadIsolated,
        WTFStringImpl__isThreadShareable,
        WTF__numberToExponential,
        WTF__numberToFixed,
        WTF__numberToPrecision,
        WTF__parseES5Date,
        __bun_crash_handler_dump_stack_trace,
        highway_copy_ascii_prefix,
        highway_encode_hex_lower,
        highway_index_of_newline_or_non_ascii,
        highway_index_of_not_char,
        highway_memmem16,
        rescle__setWindowsMetadata,
        simdutf__base64_decode_from_binary,
        simdutf__base64_decode_from_binary_lenient,
        simdutf__base64_length_from_binary,
        simdutf__convert_valid_utf16le_to_utf8,
        simdutf__utf8_length_from_latin1,
        simdutf__validate_utf16le,
        uv_close,
        uv_fs_chmod,
        uv_fs_chown,
        uv_fs_close,
        uv_fs_fchmod,
        uv_fs_fchown,
        uv_fs_fdatasync,
        uv_fs_fstat,
        uv_fs_fsync,
        uv_fs_ftruncate,
        uv_fs_lchown,
        uv_fs_link,
        uv_fs_lstat,
        uv_fs_mkdir,
        uv_fs_open,
        uv_fs_read,
        uv_fs_readlink,
        uv_fs_rename,
        uv_fs_req_cleanup,
        uv_fs_rmdir,
        uv_fs_stat,
        uv_fs_statfs,
        uv_fs_symlink,
        uv_fs_unlink,
        uv_fs_utime,
        uv_fs_write,
        uv_get_osfhandle,
        uv_guess_handle,
        uv_handle_get_type,
        uv_handle_type_name,
        uv_has_ref,
        uv_is_active,
        uv_is_closing,
        uv_loop_close,
        uv_loop_init,
        uv_open_osfhandle,
        uv_pipe,
        uv_run,
        uv_walk,
    );
}

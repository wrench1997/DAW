//! Test-only per-thread allocation guard. Worker allocations are intentionally outside its scope.
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};
thread_local! {
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
    static DROPS: Cell<usize> = const { Cell::new(0) };
}
struct CheckedAllocator;
#[global_allocator]
static ALLOCATOR: CheckedAllocator = CheckedAllocator;
unsafe impl GlobalAlloc for CheckedAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if ACTIVE.try_with(Cell::get).unwrap_or(false) {
            let _ = ALLOCS.try_with(|n| n.set(n.get() + 1));
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if ACTIVE.try_with(Cell::get).unwrap_or(false) {
            let _ = DROPS.try_with(|n| n.set(n.get() + 1));
        }
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if ACTIVE.try_with(Cell::get).unwrap_or(false) {
            let _ = ALLOCS.try_with(|n| n.set(n.get() + 1));
        }
        unsafe { System.realloc(ptr, layout, size) }
    }
}
pub fn assert_no_alloc_or_drop<T>(operation: impl FnOnce() -> T) -> T {
    ALLOCS.with(|n| n.set(0));
    DROPS.with(|n| n.set(0));
    ACTIVE.with(|active| assert!(!active.replace(true)));
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            ACTIVE.with(|active| active.set(false));
        }
    }
    let reset = Reset;
    let result = operation();
    drop(reset);
    assert_eq!(ALLOCS.with(Cell::get), 0, "callback allocated");
    assert_eq!(DROPS.with(Cell::get), 0, "callback deallocated");
    result
}

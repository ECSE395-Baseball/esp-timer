//! Debug instrumentation: trace large heap allocations.
//!
//! A global-allocator wrapper that logs every Rust allocation/realloc
//! >= [`MIN_TRACE`] bytes (and every allocation failure) with the
//! allocating task's name and the current internal-heap state, plus a
//! raw backtrace for the request sizes under investigation.
//!
//! Logging uses `esp_rom_printf` (UART, no heap) because logging from
//! inside the allocator must never allocate — that would recurse.

use core::alloc::{GlobalAlloc, Layout};
use core::ffi::{c_char, c_void};
use core::sync::atomic::{AtomicBool, Ordering};

use esp_idf_svc::sys::{
    esp_backtrace_frame_t, esp_backtrace_get_next_frame, esp_backtrace_get_start,
    heap_caps_aligned_alloc, heap_caps_get_free_size, heap_caps_get_largest_free_block,
    pcTaskGetName, xTaskGetCurrentTaskHandle, MALLOC_CAP_8BIT, MALLOC_CAP_INTERNAL,
};

extern "C" {
    // newlib malloc/free/realloc — the same internal-RAM heap the default
    // Rust allocator uses.
    fn malloc(size: usize) -> *mut c_void;
    fn free(ptr: *mut c_void);
    fn realloc(ptr: *mut c_void, new_size: usize) -> *mut c_void;
    fn esp_rom_printf(fmt: *const c_char, ...);
}

/// Allocations at least this large get a trace line.
const MIN_TRACE: usize = 2048;

/// Whether the one-shot calibration backtrace (4612-byte rqrr
/// allocation) has fired yet.
static CALIBRATED: AtomicBool = AtomicBool::new(false);

pub struct AllocTrace;

// Safety: newlib's malloc on xtensa returns 8-byte-aligned storage; larger
// alignments go through heap_caps_aligned_alloc (same heap, free()-able).
unsafe impl GlobalAlloc for AllocTrace {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let size = layout.size();
        let ptr = unsafe {
            if layout.align() <= 8 {
                malloc(size)
            } else {
                heap_caps_aligned_alloc(layout.align(), size, MALLOC_CAP_8BIT | MALLOC_CAP_INTERNAL)
            }
        } as *mut u8;
        if size >= MIN_TRACE || ptr.is_null() {
            trace(b'+', size, ptr.is_null());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        unsafe { free(ptr as *mut c_void) };
    }

    unsafe fn realloc(&self, ptr: *mut u8, _layout: Layout, new_size: usize) -> *mut u8 {
        let out = unsafe { realloc(ptr as *mut c_void, new_size) } as *mut u8;
        if new_size >= MIN_TRACE || out.is_null() {
            trace(b'~', new_size, out.is_null());
        }
        out
    }
}

fn trace(kind: u8, size: usize, failed: bool) {
    let caps = MALLOC_CAP_8BIT | MALLOC_CAP_INTERNAL;
    // Safety: read-only heap stats; both are safe from task context.
    let (free, max) = unsafe {
        (
            heap_caps_get_free_size(caps),
            heap_caps_get_largest_free_block(caps),
        )
    };
    // Safety: NULL handle = calling task; the returned name is a
    // NUL-terminated string in the TCB.
    let task = unsafe { xTaskGetCurrentTaskHandle() };
    let name = unsafe { pcTaskGetName(task) };
    // Safety: format string is a static NUL-terminated literal; arguments
    // match its specifiers. esp_rom_printf does not allocate.
    unsafe {
        esp_rom_printf(
            b"[alloc] %c %u B%s task=%s@%08x free=%u max=%u\n\0".as_ptr() as *const c_char,
            kind as u32,
            size as u32,
            if failed {
                b" FAILED\0".as_ptr() as *const c_char
            } else {
                b"\0".as_ptr() as *const c_char
            },
            name,
            task as u32,
            free as u32,
            max as u32,
        );
    }
    // Backtrace the sizes under investigation (8440 B, plus the 4612 B
    // rqrr region-cache request once as a calibration sample).
    let calibrate = size == 4612 && !CALIBRATED.swap(true, Ordering::Relaxed);
    if failed || size == 8440 || calibrate {
        backtrace();
    }
}

/// Print a raw backtrace of the allocation site (hex PCs; symbolize
/// offline with xtensa-esp32s3-elf-addr2line).
#[inline(never)]
fn backtrace() {
    let mut frame = esp_backtrace_frame_t {
        pc: 0,
        sp: 0,
        next_pc: 0,
        exc_frame: core::ptr::null(),
    };
    // Safety: fills in the first frame of the current stack (implemented
    // in IDF assembly; flushes the register windows onto the stack).
    unsafe {
        esp_backtrace_get_start(&mut frame.pc, &mut frame.sp, &mut frame.next_pc);
    }
    let mut pcs = [0u32; 10];
    pcs[0] = frame.pc;
    let mut n = 1;
    // Safety: the frame walker only reads memory below the live stack.
    while n < pcs.len() && unsafe { esp_backtrace_get_next_frame(&mut frame) } {
        pcs[n] = frame.pc;
        n += 1;
    }
    // Safety: static NUL-terminated format; ten %x args match pcs.
    unsafe {
        esp_rom_printf(
            b"[alloc] bt: %x %x %x %x %x %x %x %x %x %x\n\0".as_ptr() as *const c_char,
            pcs[0],
            pcs[1],
            pcs[2],
            pcs[3],
            pcs[4],
            pcs[5],
            pcs[6],
            pcs[7],
            pcs[8],
            pcs[9],
        );
    }
}

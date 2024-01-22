#![no_std]
#![no_main]

use core::arch::asm;
use core::panic::PanicInfo;
use core::sync::atomic::AtomicU32;

static SOME_VALUE: AtomicU32 = AtomicU32::new(0);

#[no_mangle]
extern "C" fn _start() {
    // 1. An ADRP instruction, which writes to a register Rn.
    // 2. A load or store instruction which does not write to Rn
    // 3. <an optional additional instruction>
    // 4. A load or store (unsigned immediate) using Rn
    unsafe {
        asm!(
            "nop",
            "nop",
            "adrp x0, {global}",
            "ldr x1, [x1, #0]",
            "ldr x0, [x0, :lo12:{global}]",
            global = sym SOME_VALUE,
        );
    }
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    loop {
        unsafe {
            asm!("udf #0");
        }
    }
}

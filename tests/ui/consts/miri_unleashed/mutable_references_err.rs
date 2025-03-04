// stderr-per-bitwidth
// compile-flags: -Zunleash-the-miri-inside-of-you
#![allow(invalid_reference_casting, static_mut_ref)]

use std::sync::atomic::*;
use std::cell::UnsafeCell;

// this test ensures that our mutability story is sound

struct Meh {
    x: &'static UnsafeCell<i32>,
}
unsafe impl Sync for Meh {}

// the following will never be ok! no interior mut behind consts, because
// all allocs interned here will be marked immutable.
const MUH: Meh = Meh { //~ ERROR: mutable pointer in final value
    x: &UnsafeCell::new(42),
};

struct Synced {
    x: UnsafeCell<i32>,
}
unsafe impl Sync for Synced {}

// Make sure we also catch this behind a type-erased `dyn Trait` reference.
const SNEAKY: &dyn Sync = &Synced { x: UnsafeCell::new(42) };
//~^ ERROR: mutable pointer in final value

// Make sure we also catch mutable references in values that shouldn't have them.
static mut FOO: i32 = 0;
const SUBTLE: &mut i32 = unsafe { &mut FOO };
//~^ ERROR: it is undefined behavior to use this value
//~| static
const BLUNT: &mut i32 = &mut 42;
//~^ ERROR: mutable pointer in final value

// Check for mutable references to read-only memory.
static READONLY: i32 = 0;
static mut MUT_TO_READONLY: &mut i32 = unsafe { &mut *(&READONLY as *const _ as *mut _) };
//~^ ERROR: it is undefined behavior to use this value
//~| pointing to read-only memory

// Check for consts pointing to mutable memory.
// Currently it's not even possible to create such a const.
static mut MUTABLE: i32 = 42;
const POINTS_TO_MUTABLE1: &i32 = unsafe { &MUTABLE };
//~^ ERROR: undefined behavior to use this value
//~| pointing to a static
static mut MUTABLE_REF: &mut i32 = &mut 42;
const POINTS_TO_MUTABLE2: &i32 = unsafe { &*MUTABLE_REF };
//~^ ERROR: evaluation of constant value failed
//~| accesses static

const POINTS_TO_MUTABLE_INNER: *const i32 = &mut 42 as *mut _ as *const _;
//~^ ERROR: mutable pointer in final value
const POINTS_TO_MUTABLE_INNER2: *const i32 = &mut 42 as *const _;
//~^ ERROR: mutable pointer in final value
const INTERIOR_MUTABLE_BEHIND_RAW: *mut i32 = &UnsafeCell::new(42) as *const _ as *mut _;
//~^ ERROR: mutable pointer in final value

struct SyncPtr<T> { x : *const T }
unsafe impl<T> Sync for SyncPtr<T> {}

// These pass the lifetime checks because of the "tail expression" / "outer scope" rule.
// (This relies on `SyncPtr` being a curly brace struct.)
// However, we intern the inner memory as read-only, so this must be rejected.
// (Also see `static-no-inner-mut` for similar tests on `static`.)
const RAW_SYNC: SyncPtr<AtomicI32> = SyncPtr { x: &AtomicI32::new(42) };
//~^ ERROR mutable pointer in final value
const RAW_MUT_CAST: SyncPtr<i32> = SyncPtr { x : &mut 42 as *mut _ as *const _ };
//~^ ERROR mutable pointer in final value
const RAW_MUT_COERCE: SyncPtr<i32> = SyncPtr { x: &mut 0 };
//~^ ERROR mutable pointer in final value

fn main() {
    unsafe {
        *MUH.x.get() = 99;
    }
}

// ferrocene-annotations: fls_qztk0bkju9u
// Borrow Expression
//
// ferrocene-annotations: fls_a14slch83hzn
// Borrowing
//
// ferrocene-annotations: fls_v5x85lt5ulva
// References

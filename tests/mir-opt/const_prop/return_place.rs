// unit-test: GVN
// EMIT_MIR_FOR_EACH_PANIC_STRATEGY
// compile-flags: -C overflow-checks=on

// EMIT_MIR return_place.add.GVN.diff
// EMIT_MIR return_place.add.PreCodegen.before.mir
fn add() -> u32 {
    // CHECK-LABEL: fn add(
    // CHECK: _0 = const 4_u32;
    2 + 2
}

fn main() {
    add();
}

// ferrocene-annotations: um_rustc_C_overflow_checks

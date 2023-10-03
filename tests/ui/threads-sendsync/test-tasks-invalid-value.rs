// This checks that RUST_TEST_THREADS not being 1, 2, ... is detected
// properly.

// run-fail
// error-pattern:should be a positive integer
// compile-flags: --test
// exec-env:RUST_TEST_THREADS=foo
// ignore-emscripten

#[test]
fn do_nothing() {}

// ferrocene-annotations: um_rustc_test

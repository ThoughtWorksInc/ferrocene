// compile-flags: --diagnostic-width=20 --error-format=json

// This test checks that `-Z output-width` effects the JSON error output by restricting it to an
// arbitrarily low value so that the effect is visible.

fn main() {
    let _: () = 42;
    //~^ ERROR arguments to this function are incorrect
}

// ferrocene-annotations: um_rustc_error_format

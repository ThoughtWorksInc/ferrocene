// compile-flags: --error-format human-annotate-rs -Z unstable-options

pub fn main() {
    let x: Iter; //~ ERROR cannot find type `Iter` in this scope
}

// ferrocene-annotations: um_rustc_error_format

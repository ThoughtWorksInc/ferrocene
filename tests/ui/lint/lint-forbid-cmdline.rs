// compile-flags: -F deprecated

#[allow(deprecated)] //~ ERROR allow(deprecated) incompatible
fn main() {
}

// ferrocene-annotations: um_rustc_F

trait Foo {
    fn dummy(&self) { }
    fn bar();
}

impl Foo for isize {
    fn bar(&self) {}
    //~^ ERROR method `bar` has a `&self` declaration in the impl, but not in the trait
}

fn main() {}

// ferrocene-annotations: fls_fk2m2irwpeof
// Implementations
//
// ferrocene-annotations: fls_e1pgdlv81vul
// Implementation Conformance

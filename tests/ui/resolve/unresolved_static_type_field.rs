fn f(_: bool) {}

struct Foo {
    cx: bool,
}

impl Foo {
    fn bar() {
        f(cx);
        //~^ ERROR cannot find value `cx` in this scope
    }
}

fn main() {}

// ferrocene-annotations: fls_kgbi26212eof
// Self Scope

// Test that `dyn Bar<Item = XX>` uses `'static` as the default object
// lifetime bound for the type `XX`.

trait Foo<'a> {
    type Item: ?Sized;

    fn item(&self) -> Box<Self::Item> { panic!() }
}

trait Bar { }

fn is_static<T>(_: T) where T: 'static { }

// Here, we should default to `dyn Bar + 'static`, but the current
// code forces us into a conservative, hacky path.
fn bar(x: &str) -> &dyn Foo<Item = dyn Bar> { &() }
//~^ ERROR please supply an explicit bound
//~| ERROR `(): Foo<'_>` is not satisfied

fn main() {
    let s = format!("foo");
    let r = bar(&s);
    is_static(r.item());
}

// ferrocene-annotations: fls_vhpwge5123cm
// Generic Parameters
//
// ferrocene-annotations: fls_qa98qdi42orq
// Trait Object Types
//
// ferrocene-annotations: fls_7nv8ualeaqe3
// Where Clauses

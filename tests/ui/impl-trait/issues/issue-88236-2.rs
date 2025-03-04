// this used to cause stack overflows

trait Hrtb<'a> {
    type Assoc;
}

impl<'a> Hrtb<'a> for () {
    type Assoc = ();
}

impl<'a> Hrtb<'a> for &'a () {
    type Assoc = ();
}

fn make_impl() -> impl for<'a> Hrtb<'a, Assoc = impl Send + 'a> {}
//~^ ERROR higher kinded lifetime bounds on nested opaque types are not supported yet

fn make_weird_impl<'b>(x: &'b ()) -> impl for<'a> Hrtb<'a, Assoc = impl Send + 'a> {
    //~^ ERROR higher kinded lifetime bounds on nested opaque types are not supported yet
    &()
    //~^ ERROR implementation of `Hrtb` is not general enough
    //~| ERROR implementation of `Hrtb` is not general enough
}

fn make_bad_impl<'b>(x: &'b ()) -> impl for<'a> Hrtb<'a, Assoc = impl Send + 'a> {
    //~^ ERROR higher kinded lifetime bounds on nested opaque types are not supported yet
    x
    //~^ ERROR implementation of `Hrtb` is not general enough
    //~| ERROR implementation of `Hrtb` is not general enough
    //~| ERROR lifetime may not live long enough
}

fn main() {}

// ferrocene-annotations: fls_jeoas4n6su4
// Trait and Lifetime Bounds

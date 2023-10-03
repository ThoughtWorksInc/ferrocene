// Emit additional suggestion to correct the trait implementation
// on a pointer
use std::{fmt, marker};

struct LocalType;

impl fmt::Display for *mut LocalType {
//~^ ERROR only traits defined in the current crate can be implemented for arbitrary types
//~| NOTE impl doesn't use only types from inside the current crate
//~| NOTE `*mut LocalType` is not defined in the current crate because raw pointers are always foreign
//~| NOTE define and implement a trait or new type instead
//~| HELP consider introducing a new wrapper type
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "This not compile")
    }
}

impl<T> marker::Copy for *mut T {
//~^ ERROR only traits defined in the current crate can be implemented for arbitrary types
//~| NOTE impl doesn't use only types from inside the current crate
//~| NOTE `*mut T` is not defined in the current crate because raw pointers are always foreign
//~| NOTE define and implement a trait or new type instead
}

fn main() {}

// ferrocene-annotations: fls_46ork6fz5o2e
// Implementation Coherence

// run-pass

#![allow(dead_code)]
#![allow(unused_variables)]

// Some traits can be derived for unions.

#[derive(
    Copy,
    Clone,
    Eq,
)]
union U {
    a: u8,
    b: u16,
}

impl PartialEq for U { fn eq(&self, rhs: &Self) -> bool { true } }

#[derive(
    Clone,
    Copy,
    Eq
)]
union W<T: Copy> {
    a: T,
}

impl<T: Copy> PartialEq for W<T> { fn eq(&self, rhs: &Self) -> bool { true } }

fn main() {
    let u = U { b: 0 };
    let u1 = u;
    let u2 = u.clone();
    assert!(u1 == u2);

    let w = W { a: 0 };
    let w1 = w.clone();
    assert!(w == w1);
}

// ferrocene-annotations: fls_r6gj1p4gajnq
// Attribute derive
//
// ferrocene-annotations: fls_fk2m2irwpeof
// Implementations
//
// ferrocene-annotations: fls_z7q8kbjwdc7g
// Method Call Expressions
//
// ferrocene-annotations: fls_wqazkzle0ix9
// Method Resolution

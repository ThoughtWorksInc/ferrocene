//! Tests for RFC 3086 metavariable expressions.

use expect_test::expect;

use crate::macro_expansion_tests::check;

#[test]
fn test_dollar_dollar() {
    check(
        r#"
macro_rules! register_struct { ($Struct:ident) => {
    macro_rules! register_methods { ($$($method:ident),*) => {
        macro_rules! implement_methods { ($$$$($$val:expr),*) => {
            struct $Struct;
            impl $Struct { $$(fn $method() -> &'static [u32] { &[$$$$($$$$val),*] })*}
        }}
    }}
}}

register_struct!(Foo);
register_methods!(alpha, beta);
implement_methods!(1, 2, 3);
"#,
        expect![[r#"
macro_rules! register_struct { ($Struct:ident) => {
    macro_rules! register_methods { ($$($method:ident),*) => {
        macro_rules! implement_methods { ($$$$($$val:expr),*) => {
            struct $Struct;
            impl $Struct { $$(fn $method() -> &'static [u32] { &[$$$$($$$$val),*] })*}
        }}
    }}
}}

macro_rules !register_methods {
    ($($method: ident), *) = > {
        macro_rules!implement_methods {
            ($$($val: expr), *) = > {
                struct Foo;
                impl Foo {
                    $(fn $method()-> &'static[u32] {
                        &[$$($$val), *]
                    }
                    )*
                }
            }
        }
    }
}
macro_rules !implement_methods {
    ($($val: expr), *) = > {
        struct Foo;
        impl Foo {
            fn alpha()-> &'static[u32] {
                &[$($val), *]
            }
            fn beta()-> &'static[u32] {
                &[$($val), *]
            }
        }
    }
}
struct Foo;
impl Foo {
    fn alpha() -> &'static[u32] {
        &[1, 2, 3]
    }
    fn beta() -> &'static[u32] {
        &[1, 2, 3]
    }
}
"#]],
    )
}

#[test]
fn test_metavar_exprs() {
    check(
        r#"
macro_rules! m {
    ( $( $t:tt )* ) => ( $( ${ignore($t)} -${index()} )-* );
}
const _: i32 = m!(a b c);
    "#,
        expect![[r#"
macro_rules! m {
    ( $( $t:tt )* ) => ( $( ${ignore($t)} -${index()} )-* );
}
const _: i32 = -0--1--2;
    "#]],
    );
}

#[test]
fn count_basic() {
    check(
        r#"
macro_rules! m {
    ($($t:ident),*) => {
        ${count($t)}
    }
}

fn test() {
    m!();
    m!(a);
    m!(a, a);
}
"#,
        expect![[r#"
macro_rules! m {
    ($($t:ident),*) => {
        ${count($t)}
    }
}

fn test() {
    0;
    1;
    2;
}
"#]],
    );
}

#[test]
fn count_with_depth() {
    check(
        r#"
macro_rules! foo {
    ($( $( $($t:ident)* ),* );*) => {
        $(
            {
                let depth_none = ${count($t)};
                let depth_zero = ${count($t, 0)};
                let depth_one = ${count($t, 1)};
            }
        )*
    }
}

fn bar() {
    foo!(
        a a a, a, a a;
        a a a
    )
}
"#,
        expect![[r#"
macro_rules! foo {
    ($( $( $($t:ident)* ),* );*) => {
        $(
            {
                let depth_none = ${count($t)};
                let depth_zero = ${count($t, 0)};
                let depth_one = ${count($t, 1)};
            }
        )*
    }
}

fn bar() {
     {
        let depth_none = 3;
        let depth_zero = 3;
        let depth_one = 6;
    } {
        let depth_none = 1;
        let depth_zero = 1;
        let depth_one = 3;
    }
}
"#]],
    );
}

#[test]
fn count_depth_out_of_bounds() {
    check(
        r#"
macro_rules! foo {
    ($($t:ident)*) => { ${count($t, 1)} };
    ($( $( $l:literal )* );*) => { $(${count($l, 1)};)* }
}
macro_rules! bar {
    ($($t:ident)*) => { ${count($t, 1024)} };
    ($( $( $l:literal )* );*) => { $(${count($l, 8192)};)* }
}

fn test() {
    foo!(a b);
    foo!(1 2; 3);
    bar!(a b);
    bar!(1 2; 3);
}
"#,
        expect![[r#"
macro_rules! foo {
    ($($t:ident)*) => { ${count($t, 1)} };
    ($( $( $l:literal )* );*) => { $(${count($l, 1)};)* }
}
macro_rules! bar {
    ($($t:ident)*) => { ${count($t, 1024)} };
    ($( $( $l:literal )* );*) => { $(${count($l, 8192)};)* }
}

fn test() {
    2;
    2;
    1;;
    2;
    2;
    1;;
}
"#]],
    );
}

#[test]
fn misplaced_count() {
    check(
        r#"
macro_rules! foo {
    ($($t:ident)*) => { $(${count($t)})* };
    ($l:literal) => { ${count($l)} }
}

fn test() {
    foo!(a b c);
    foo!(1);
}
"#,
        expect![[r#"
macro_rules! foo {
    ($($t:ident)*) => { $(${count($t)})* };
    ($l:literal) => { ${count($l)} }
}

fn test() {
    1 1 1;
    1;
}
"#]],
    );
}

#[test]
fn malformed_count() {
    check(
        r#"
macro_rules! too_many_args {
    ($($t:ident)*) => { ${count($t, 1, leftover)} }
}
macro_rules! depth_suffixed {
    ($($t:ident)*) => { ${count($t, 0usize)} }
}
macro_rules! depth_too_large {
    ($($t:ident)*) => { ${count($t, 18446744073709551616)} }
}

fn test() {
    too_many_args!();
    depth_suffixed!();
    depth_too_large!();
}
"#,
        expect![[r#"
macro_rules! too_many_args {
    ($($t:ident)*) => { ${count($t, 1, leftover)} }
}
macro_rules! depth_suffixed {
    ($($t:ident)*) => { ${count($t, 0usize)} }
}
macro_rules! depth_too_large {
    ($($t:ident)*) => { ${count($t, 18446744073709551616)} }
}

fn test() {
    /* error: invalid macro definition: invalid metavariable expression */;
    /* error: invalid macro definition: invalid metavariable expression */;
    /* error: invalid macro definition: invalid metavariable expression */;
}
"#]],
    );
}

#[test]
fn count_interaction_with_empty_binding() {
    // FIXME: Should this error? rustc currently accepts it.
    check(
        r#"
macro_rules! m {
    ($($t:ident),*) => {
        ${count($t, 100)}
    }
}

fn test() {
    m!();
}
"#,
        expect![[r#"
macro_rules! m {
    ($($t:ident),*) => {
        ${count($t, 100)}
    }
}

fn test() {
    0;
}
"#]],
    );
}

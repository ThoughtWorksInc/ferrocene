fn main() {
    let a = if true {
        0
    } else if false {
//~^ ERROR `if` may be missing an `else` clause
//~| expected integer, found `()`
        1
    };
}

// ferrocene-annotations: fls_mkut7gut49gi
// If Expressions

fn main() {
    [(); &(&'static: loop { |x| {}; }) as *const _ as usize]
    //~^ ERROR: invalid label name `'static`
    //~| ERROR: type annotations needed
}

// ferrocene-annotations: fls_dw33yt5g6m0k
// Type Coercion
//
// ferrocene-annotations: fls_exe4zodlwfez
// Type Unification

extern crate core;

fn assert_send<T: Send>() {}

fn test70() {
    assert_send::<*mut isize>();
    //~^ ERROR `*mut isize` cannot be sent between threads safely
}

fn test71<'a>() {
    assert_send::<*mut &'a isize>();
    //~^ ERROR `*mut &'a isize` cannot be sent between threads safely
}

fn main() {}

// ferrocene-annotations: fls_eiw4by8z75di
// Send and Sync

#![cfg_attr(feature = "clippy", feature(plugin))]
#![cfg_attr(feature = "clippy", plugin(clippy))]
#[macro_use]
pub mod maptable;
pub mod storage;
pub mod util;
extern crate crossbeam_deque;
extern crate crossbeam_epoch as epoch;
extern crate crossbeam_utils as utils;
extern crate libc;
#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}

#![cfg_attr(feature = "clippy", feature(plugin))]
#![cfg_attr(feature = "clippy", plugin(clippy))]
#![feature(alloc)]
#![feature(align_offset)]
#![feature(offset_to)]
#![feature(raw_vec_internals)]
#[macro_use]
pub mod maptable;
pub mod db;
pub mod env;
pub mod util;
extern crate alloc;
extern crate crossbeam_deque;
extern crate crossbeam_epoch as epoch;
extern crate crossbeam_utils as utils;
extern crate libc;
use env::SequentialFile;
use env::WritableFile;
#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}

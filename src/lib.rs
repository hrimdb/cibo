#![cfg_attr(feature = "clippy", feature(plugin))]
#![cfg_attr(feature = "clippy", plugin(clippy))]
#![feature(alloc)]
#![feature(ptr_offset_from)]
#![feature(raw_vec_internals)]
#[macro_use]
pub mod maptable;
pub mod db;
pub mod env;
pub mod util;




#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}

#![feature(non_null_from_ref)]

mod boc;
mod r#ref;

#[cfg(feature = "rayon")]
use rayon as runtime;
#[cfg(not(any(feature = "rayon")))]
use std::thread as runtime;

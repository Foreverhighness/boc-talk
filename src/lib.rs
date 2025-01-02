#![feature(non_null_from_ref)]

mod boc;
mod r#ref;

#[cfg(not(any(feature = "rayon")))]
use std::thread as runtime;

#[cfg(feature = "rayon")]
use rayon as runtime;

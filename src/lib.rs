#![feature(non_null_from_ref)]

#[cfg_attr(feature = "ref", path = "ref.rs")]
#[cfg_attr(not(feature = "ref"), path = "boc.rs")]
mod imp;

#[cfg(not(any(feature = "rayon")))]
use std::thread as runtime;

pub use imp::{CownPtr, run_when};
#[cfg(feature = "rayon")]
use rayon as runtime;

#[cfg(test)]
mod tests;

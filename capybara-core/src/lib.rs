#![allow(dead_code)]
#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(unused_assignments)]
#![allow(clippy::type_complexity)]
#![allow(clippy::from_over_into)]
#![allow(clippy::module_inception)]
#![allow(clippy::upper_case_acronyms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

#[macro_use]
extern crate anyhow;
#[macro_use]
extern crate cfg_if;
#[macro_use]
extern crate log;
extern crate string_cache;

pub use builtin::setup;
pub use error::CapybaraError;
pub use upstream::{Pool, Pools, RoundRobinPools, WeightedPools};

pub type Result<T> = std::result::Result<T, CapybaraError>;

/// cached string
pub mod cachestr {
    include!(concat!(env!("OUT_DIR"), "/cachestr.rs"));
}

mod builtin;
mod error;
mod logger;
mod macros;
pub mod pipeline;
pub mod proto;
pub mod protocol;
pub mod resolver;
pub mod transport;
mod upstream;

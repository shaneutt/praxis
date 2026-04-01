//! Configuration validation test suite for Praxis.
//!
//! Exercises [`Config::from_yaml`] against valid and invalid YAML inputs to
//! verify every validation rule in `praxis-core`.
//!
//! [`Config::from_yaml`]: praxis_core::config::Config::from_yaml

mod cluster;
mod cross_cutting;
mod edge_cases;
mod filter_chain;
mod helpers;
mod listener;
mod route;

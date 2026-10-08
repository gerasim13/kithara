//! Parallel execution chains: a decision whose arms lead into alike chains of
//! functions that only that arm calls.

mod aliases;
mod arms;
mod body;
mod chain;
mod config;
#[cfg(test)]
mod corpus;
mod facts;
mod graph;
mod minhash;
mod pairs;
mod report;
mod resolve;
mod search;
mod standard;
#[cfg(test)]
mod tests;
mod ty;

pub(crate) use self::{
    config::ChainConfig,
    report::{ChainReport, markdown},
    search::detect,
};

//! Parallel execution chains: a decision whose arms lead into alike chains of
//! functions that only that arm calls.

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
#[cfg(test)]
mod tests;

pub(crate) use self::{
    config::ChainConfig,
    report::{ChainReport, markdown},
    search::detect,
};

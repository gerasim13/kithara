mod core;
pub(crate) mod persistence;

pub(crate) use core::{AnalysisTarget, TrackAnalysisCache, token_for};

pub(crate) use persistence::{AnalysisPersistence, AnalysisPersistenceError};

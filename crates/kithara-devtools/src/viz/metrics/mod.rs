mod analyzer;

use super::graph::NodeId;

mod topology;

pub(crate) use analyzer::{ArchitectureMetrics, MetricsAnalyzer};
use analyzer::{Relation, ratio_or_zero};

//! Core types for Project Watcher: check states, metrics, units, thresholds,
//! check output parsing and result evaluation.

mod eval;
mod metric;
mod num;
pub mod parse;
mod service;
mod state;
mod threshold;
mod unit;

pub use eval::{CheckResult, MetricResult, RawOutput, Termination, ThresholdSpec, evaluate};
pub use metric::Metric;
pub use parse::{Format, ParseError, ParsedOutput, parse_output};
pub use service::{
    FLAP_WINDOW, FlapHistory, FlapThresholds, HostChange, ServiceState, StateChange, StateType,
};
pub use state::{HostState, State};
pub use threshold::{CmpOp, ConfigThreshold, Threshold, ThresholdError};
pub use unit::{Unit, format_value};

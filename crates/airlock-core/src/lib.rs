//! Airlock core: the types every other crate speaks.
//!
//! Data flow (see docs/ARCHITECTURE.md):
//! ingest -> [analysis taps produce `Detection`s] -> `DecisionEngine` turns them into
//! `Redaction`s per `Policy` -> output stage asks the engine what to redact for each
//! chunk as it leaves the `DelayLine`.

pub mod buffer;
pub mod decision;
pub mod error;
pub mod model;
pub mod policy;
pub mod time;

pub use buffer::{DelayLine, DelayRamp};
pub use decision::{Action, DecisionEngine, Redaction, RedactionId, Status, Target};
pub use error::{Error, Result};
pub use model::{Category, Detection, DetectorKind, Modality, Platform, Rect, TextBox, Word};
pub use policy::{AudioAction, CategoryPolicy, Mode, Policy, StallFallback, VideoAction};
pub use time::{Dur, Span, Ts};

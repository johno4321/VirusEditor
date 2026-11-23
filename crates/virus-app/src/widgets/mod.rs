//! Custom UI widgets for the Access Virus Editor.

pub mod curves;
pub mod knob;

pub use curves::{EnvelopeCurveWidget, FilterCurveWidget};
pub use knob::{bipolar_knob, unipolar_knob};

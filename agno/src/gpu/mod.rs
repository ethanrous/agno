//! GPU acceleration support module.
//!
//! This module provides GPU context management and compute dispatch
//! for image processing operations when the `gpu` feature is enabled.

pub mod context;

pub use context::GpuContext;

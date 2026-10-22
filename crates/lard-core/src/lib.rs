//! Shared, dependency-light primitives for the whole suite.
//!
//! This crate intentionally has no Wayland, GPU or async dependency: it can be
//! unit-tested on any machine and keeps the shell layers free of circular
//! dependencies.

pub mod anim;
pub mod color;
pub mod config;
pub mod desktop;
pub mod error;
pub mod geometry;
pub mod theme;

pub use color::Color;
pub use config::Config;
pub use error::ConfigError;
pub use geometry::{Point, Rect, Size};

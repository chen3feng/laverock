//! Per-family request shaping for the audio.cpp runtime.
//!
//! `mod.rs` owns the shared CLI/resident plumbing; each file here maps one
//! manifest family onto its CLI flags and resident-server options.

pub(super) mod ace_step;
pub(super) mod cosyvoice;
pub(super) mod dots;
pub(super) mod indextts2;
pub(super) mod voxcpm2;

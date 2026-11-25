//! TableMaker: build wavetables out of audio clips of one source.
//!
//! Each clip is oversampled so its rising zero crossings can be located to
//! a small fraction of a sample. Crossings one tracked pitch period apart
//! bound candidate single cycles; the ones that repeat in the next period
//! and loop without a slope mismatch are kept and rendered, band-limited,
//! to the frame size. Cycles from every clip are then mashed into one
//! table: selected, ordered for smooth morphing, phase-aligned and filled
//! in between.

pub mod cycles;
pub mod envelope;
pub mod formant;
pub mod lpc;
pub mod pitch;
pub mod preview;
pub mod report;
pub mod resample;
pub mod signal;
pub mod table;
#[doc(hidden)]
pub mod testsignal;
pub mod wav;

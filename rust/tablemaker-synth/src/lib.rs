//! The voice synthesizer: plays a speaker's vowel table at any pitch with
//! the formants where they were recorded, morphing between vowels, with the
//! speaker's own vibrato, pitch and level wander, and breathiness.

pub mod bank;
pub mod engine;

pub use bank::Bank;
pub use engine::{Engine, Event, Params};

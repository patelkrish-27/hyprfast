//! Dedicated Decider client for hyprfast.
//!
//! Talks only to the external resident decider service (no local model,
//! no Python subprocess). See `config`, `types`, `client` for details.

pub mod candidates;
pub mod client;
pub mod config;
pub mod hint_resolve;
pub mod image;
pub mod types;
pub mod tools;
pub mod resolve {
    pub use crate::perception::resolve::*;
}

pub use candidates::{
    Candidate, CandidateFilter, Candidates, Rect, TEXT_MAX_OPTIONS, VISION_MAX_OPTIONS,
};
pub use client::{metrics, DeciderClient, DeciderMetrics};
pub use config::DeciderConfig;
pub use image::{ImageSource, ResizeInfo};
pub use types::{
    strip_data_uri, validate_image_field, DeciderDecision, DeciderQuestion, DeciderRequest,
    DeciderResponse,
};
pub use hint_resolve::{
    hint_resolve, hint_resolve_async, hint_resolve_batch, hint_resolve_batch_async,
    hint_resolve_with_target, hint_resolve_vision, key_identify, key_identify_async, key_identify_sync,
};

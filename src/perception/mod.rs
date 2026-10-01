//! Perception layer — re-exports Decider candidate abstraction + image pipeline.
//!
//! The task requested `src/perception/` or `src/decider/` for the abstraction.
//! This crate exposes both paths: canonical logic lives in `crate::decider::candidates`
//! and `crate::decider::image`; this module re-exports for ergonomic `perception::` access.

pub use crate::decider::candidates::{
    Candidate, CandidateFilter, Candidates, Rect, TEXT_MAX_OPTIONS, VISION_MAX_OPTIONS,
};
pub use crate::decider::image::{
    annotate_and_encode, annotate_screenshot, annotation_legend, capture_with_source,
    decode_image_field, detect_dimensions, detect_format, encode_base64, encode_for_decider,
    max_image_dim_from_env, maybe_resize, pipeline, pipeline_for_candidate, to_data_uri,
    to_data_uri_auto, vision_pipeline, ImageSource, ResizeInfo,
};

pub mod resolve;
pub mod verify;
pub mod tools;

pub use resolve::{
    ResolveContext, ResolveResult, ResolveStatus, ResolveTier,
    collect_candidates, exact_match, heuristic_single_match,
    resolve, resolve_async, resolve_batch, resolve_batch_async,
    resolve_target, resolve_target_async,
};
pub use verify::{
    VerifyResult, VerifyStatus,
    verify, verify_async, verify_element, verify_element_async,
    verify_action, verify_action_async, wait_until, wait_until_async,
};

// Convenience alias so `perception::candidates` and `perception::image` also work
pub mod candidates {
    pub use crate::decider::candidates::*;
}
pub mod image {
    pub use crate::decider::image::*;
}

//! Perception tools — centralizes semantic helpers reusing the Decider resolver.
//! Canonical implementations live in `crate::decider::tools`; this module
//! re-exports and adds perception-named aliases so callers may use either path
//! without incompatible duplicates.

pub use crate::decider::tools::{
    // low-level
    decide, decide_async, decider_batch, decider_batch_async,
    // semantic
    find, find_async, choose, choose_async, classify, classify_async,
    detect, detect_async, identify, identify_async, visual_target, visual_target_async,
    // verify
    verify, verify_async_tool as verify_async, verify_element, verify_element_async_tool as verify_element_async,
    verify_action, verify_action_async_tool as verify_action_async,
    wait_until, wait_until_async_tool as wait_until_async,
    observe_state, observe_state_async,
    // hint/key
    hint_resolve, hint_resolve_async_tool as hint_resolve_async,
    hint_resolve_batch, hint_resolve_batch_async_tool as hint_resolve_batch_async,
    key_identify, key_identify_async_tool as key_identify_async,
    // composites
    find_and_click, find_and_click_async, find_and_type, find_and_type_async,
};

// Backwards-compat: some callers import `perception::tools::resolve` helpers
pub use crate::perception::resolve::{ResolveContext, ResolveResult, ResolveStatus, ResolveTier, resolve, resolve_async, resolve_target, resolve_target_async, resolve_batch, resolve_batch_async};
pub use crate::perception::verify::{VerifyResult, VerifyStatus, verify as verify_perception, verify_element as verify_element_perception, verify_action as verify_action_perception, wait_until as wait_until_perception};

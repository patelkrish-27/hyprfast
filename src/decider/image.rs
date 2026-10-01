//! Image pipeline for Decider-2B integration.
//!
//! Reuses existing screenshot infrastructure only:
//! - `crate::screenshot::capture` (grim screencopy, JPEG/PNG)
//! - `crate::browser::screenshot_cdp` (Page.captureScreenshot, PNG)
//!
//! No second screenshot mechanism is introduced.
//!
//! Pipeline:
//!   existing capture -> optional region selection -> optional resize
//!   -> JPEG/PNG encoding (already done by capture) -> base64 / data URI -> Decider.
//!
//! Resize respects `DECIDER_MAX_IMAGE_DIM` (default 1280) deterministically.
//! Without the `image` crate, resize is reported as `would_resize` and the
//! original bytes are forwarded — the server should downscale. When the `image`
//! crate is available (feature-gated), a cheap nearest-neighbor / triangle
//! resize is applied. Annotate helpers draw numeric labels + bounding boxes
//! non-destructively; fallback is JSON legend overlay (no pixel mutation).

use anyhow::Result;
use base64::Engine;
use serde::Serialize;
use serde_json::{json, Value};

use crate::decider::candidates::{Candidates, Rect};

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Read `DECIDER_MAX_IMAGE_DIM` from env, default 1280.
pub fn max_image_dim_from_env() -> u32 {
    std::env::var("DECIDER_MAX_IMAGE_DIM")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1280)
}

/// From `DeciderConfig`.
pub fn max_image_dim_from_config(cfg: &crate::decider::config::DeciderConfig) -> u32 {
    cfg.max_image_dim
}

// ---------------------------------------------------------------------------
// Format / dimension detection (no image crate)
// ---------------------------------------------------------------------------

/// Detect mime format from magic bytes.
pub fn detect_format(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(&[0xFF, 0xD8]) {
        "jpeg"
    } else if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        "png"
    } else {
        "unknown"
    }
}
pub fn mime_for_format(fmt: &str) -> &'static str {
    match fmt {
        "jpeg" | "jpg" => "image/jpeg",
        "png" => "image/png",
        _ => "image/png",
    }
}

/// Parse dimensions from PNG IHDR or JPEG SOF, mirroring `screenshot::image_size`.
pub fn detect_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) && bytes.len() >= 24 {
        let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        let h = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
        return Some((w, h));
    }
    if bytes.starts_with(&[0xFF, 0xD8]) {
        let mut i = 2usize;
        while i + 9 < bytes.len() {
            if bytes[i] != 0xFF {
                i += 1;
                continue;
            }
            let m = bytes[i + 1];
            if m == 0xD8 || m == 0xD9 || (0xD0..=0xD7).contains(&m) {
                i += 2;
                continue;
            }
            if i + 3 >= bytes.len() {
                break;
            }
            let seg = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
            if [
                0xC0, 0xC1, 0xC2, 0xC3, 0xC5, 0xC6, 0xC7, 0xC9, 0xCA, 0xCB, 0xCD, 0xCE, 0xCF,
            ]
            .contains(&m)
            {
                let h = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]) as u32;
                let w = u16::from_be_bytes([bytes[i + 7], bytes[i + 8]]) as u32;
                return Some((w, h));
            }
            i += 2 + seg;
        }
    }
    None
}

pub fn needs_resize(dim: Option<(u32, u32)>, max_dim: u32) -> bool {
    if let Some((w, h)) = dim {
        w > max_dim || h > max_dim
    } else {
        false
    }
}

fn scale_for_max(dim: (u32, u32), max_dim: u32) -> f64 {
    let m = dim.0.max(dim.1) as f64;
    if m <= max_dim as f64 || m == 0.0 {
        1.0
    } else {
        max_dim as f64 / m
    }
}

// ---------------------------------------------------------------------------
// Encoding helpers
// ---------------------------------------------------------------------------

/// Encode bytes as standard base64 (no data URI prefix).
pub fn encode_base64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Encode bytes as data URI `data:<mime>;base64,<b64>`.
pub fn to_data_uri(bytes: &[u8], mime: &str) -> String {
    format!("data:{};base64,{}", mime, encode_base64(bytes))
}

/// Convenience: bytes -> data URI with format-detected mime.
pub fn to_data_uri_auto(bytes: &[u8]) -> String {
    let fmt = detect_format(bytes);
    let mime = mime_for_format(fmt);
    to_data_uri(bytes, mime)
}

/// Decode base64 or data URI to bytes (validates).
pub fn decode_image_field(s: &str) -> Result<Vec<u8>> {
    let b64 = crate::decider::types::strip_data_uri(s);
    let cleaned: String = b64.chars().filter(|c| !c.is_whitespace()).collect();
    base64::engine::general_purpose::STANDARD
        .decode(&cleaned)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(&cleaned))
        .map_err(|e| anyhow::anyhow!("base64 decode: {e}"))
}

// ---------------------------------------------------------------------------
// Resize (deterministic, cheap)
// ---------------------------------------------------------------------------

/// Result of a resize attempt.
#[derive(Debug, Clone)]
pub struct ResizeInfo {
    pub original: Option<(u32, u32)>,
    pub target: Option<(u32, u32)>,
    pub did_resize: bool,
    pub would_resize: bool,
    pub scale: f64,
    pub max_dim: u32,
}

/// Deterministic resize helper.
///
/// Without the `image` crate we cannot do true pixel scaling, so we return
/// the original bytes with `would_resize=true` when dimensions exceed
/// `max_dim`. Caller (or server) should downscale. With feature `image` we
/// would apply a cheap triangle/nearest resize.
pub fn maybe_resize(bytes: Vec<u8>, max_dim: u32) -> (Vec<u8>, ResizeInfo) {
    let orig = detect_dimensions(&bytes);
    let scale = orig.map(|d| scale_for_max(d, max_dim)).unwrap_or(1.0);
    let would_resize = scale < 1.0 - 1e-6;

    if !would_resize {
        return (
            bytes,
            ResizeInfo {
                original: orig,
                target: orig,
                did_resize: false,
                would_resize: false,
                scale: 1.0,
                max_dim,
            },
        );
    }

    // Compute target dims deterministically.
    let target = orig.map(|(w, h)| {
        let nw = ((w as f64 * scale).round() as u32).max(1);
        let nh = ((h as f64 * scale).round() as u32).max(1);
        (nw, nh)
    });

    // Without `image` crate we preserve bytes and report would_resize.
    // If `image` feature is enabled, we would do:
    //   let img = image::load_from_memory(&bytes) ...
    //   let resized = img.resize(nw, nh, image::imageops::Triangle)
    //   let mut out = Vec::new(); resized.write_to(&mut Cursor::new(&mut out), ImageFormat::Jpeg) ...
    // To keep deps minimal we avoid that here; behavior is deterministic and
    // documented via ResizeInfo. A future `image` dependency can be gated.

    (
        bytes,
        ResizeInfo {
            original: orig,
            target,
            did_resize: false,
            would_resize: true,
            scale,
            max_dim,
        },
    )
}

/// Human-readable note for resize (for meta).
fn resize_note(info: &ResizeInfo) -> Option<String> {
    if info.would_resize && !info.did_resize {
        Some(format!(
            "image {}x{} exceeds DECIDER_MAX_IMAGE_DIM={} (scale {:.3} -> {}x{}); forwarding original — server should downscale or add `image` crate for local resize",
            info.original.map(|(w, _)| w).unwrap_or(0),
            info.original.map(|(_, h)| h).unwrap_or(0),
            info.max_dim,
            info.scale,
            info.target.map(|(w, _)| w).unwrap_or(0),
            info.target.map(|(_, h)| h).unwrap_or(0),
        ))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Capture sources — unified entry, reuses only existing capture paths
// ---------------------------------------------------------------------------

/// Unified image source selector. All variants delegate to existing
/// `screenshot::capture` or `browser::screenshot_cdp` — no new grim/CDP path.
#[derive(Debug, Clone)]
pub enum ImageSource {
    /// Entire focused monitor (default grim path, no window/region).
    Monitor,
    /// Active window (`window="active"` via grim).
    ActiveWindow,
    /// Specific window by address or title substring (grim window path).
    Window(String),
    /// Explicit region string `"x,y WxH"` or `"x,y, WxH"` (grim region path).
    Region(String),
    /// Browser viewport/tab via CDP `Page.captureScreenshot` (PNG).
    Browser,
    /// Browser image with target routing not yet needed; placeholder for
    /// future tab-aware CDP (currently same as Browser).
    BrowserTarget(String),
    /// Candidate's rect (global logical coords → grim region). Requires
    /// that `candidate.rect` is in global logical space (as from hint).
    CandidateRect(Rect),
}

impl Default for ImageSource {
    fn default() -> Self {
        Self::Monitor
    }
}

impl std::fmt::Display for ImageSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Monitor => write!(f, "monitor"),
            Self::ActiveWindow => write!(f, "active-window"),
            Self::Window(s) => write!(f, "window:{s}"),
            Self::Region(s) => write!(f, "region:{s}"),
            Self::Browser => write!(f, "browser-viewport"),
            Self::BrowserTarget(s) => write!(f, "browser-target:{s}"),
            Self::CandidateRect(r) => write!(f, "candidate-rect:{}:{}:{}x{}", r.x, r.y, r.width, r.height),
        }
    }
}

/// Capture image bytes via the single allowed screenshot paths.
///
/// - Monitor / Window / Region / CandidateRect → `screenshot::capture` (grim)
/// - Browser / BrowserTarget → `browser::screenshot_cdp` (CDP)
///
/// Returns `(bytes, meta)` where `meta` mirrors existing capture meta
/// (`geometry`, `scale`, `format`, `image` dims, `target`).
pub fn capture_with_source(source: &ImageSource, scale: f64) -> Result<(Vec<u8>, Value)> {
    match source {
        ImageSource::Monitor => crate::screenshot::capture("", "", scale),
        ImageSource::ActiveWindow => crate::screenshot::capture("active", "", scale),
        ImageSource::Window(addr) => crate::screenshot::capture(addr, "", scale),
        ImageSource::Region(region) => crate::screenshot::capture("", region, scale),
        ImageSource::CandidateRect(rect) => {
            let region = format!("{},{} {}x{}", rect.x, rect.y, rect.width, rect.height);
            crate::screenshot::capture("", &region, scale)
        }
        ImageSource::Browser | ImageSource::BrowserTarget(_) => {
            // Browser path is always PNG via CDP; scale param not applicable.
            // We forward directly; caller may still apply max-dim logic.
            crate::browser::screenshot_cdp()
        }
    }
}

// ---------------------------------------------------------------------------
// Pipeline: capture -> optional resize -> encode
// ---------------------------------------------------------------------------

/// Full pipeline helper: capture from `source`, optionally downscale respecting
/// `max_dim`, then encode as base64 or data URI.
///
/// - `max_dim`: None => env default (DECIDER_MAX_IMAGE_DIM = 1280).
/// - `as_data_uri`: true => `data:image/jpeg;base64,...`, false => raw base64.
/// - `scale`: grim scale factor (0.0 = default). Ignored for Browser source.
///
/// Returns `(encoded_string, meta)` where `meta` includes resize info.
pub fn pipeline(
    source: &ImageSource,
    max_dim: Option<u32>,
    as_data_uri: bool,
    scale: f64,
) -> Result<(String, Value)> {
    let max = max_dim.unwrap_or_else(max_image_dim_from_env);
    let (bytes, mut meta) = capture_with_source(source, scale)?;
    let fmt = detect_format(&bytes);
    let (out_bytes, info) = maybe_resize(bytes, max);

    // Record resize info in meta.
    let mut pipeline_meta = json!({
        "source": source.to_string(),
        "format": fmt,
        "max_dim": max,
        "original_dims": info.original.map(|(w,h)| json!([w,h])).unwrap_or(Value::Null),
        "target_dims": info.target.map(|(w,h)| json!([w,h])).unwrap_or(Value::Null),
        "scale": info.scale,
        "would_resize": info.would_resize,
        "did_resize": info.did_resize,
        "scale_param": scale,
    });
    if let Some(note) = resize_note(&info) {
        pipeline_meta["note"] = Value::String(note);
    }
    // Merge with capture meta
    if let Some(obj) = pipeline_meta.as_object() {
        for (k, v) in obj {
            meta[k.clone()] = v.clone();
        }
    }
    meta["pipeline"] = pipeline_meta;

    let encoded = if as_data_uri {
        let mime = mime_for_format(detect_format(&out_bytes));
        to_data_uri(&out_bytes, mime)
    } else {
        encode_base64(&out_bytes)
    };
    Ok((encoded, meta))
}

/// Simpler variant: already have bytes (e.g. from explicit `screenshot::capture` call
/// elsewhere). Apply optional resize then encode.
pub fn encode_for_decider(
    bytes: Vec<u8>,
    max_dim: Option<u32>,
    as_data_uri: bool,
) -> Result<(String, Value)> {
    let max = max_dim.unwrap_or_else(max_image_dim_from_env);
    let fmt = detect_format(&bytes);
    let (out_bytes, info) = maybe_resize(bytes, max);
    let mut meta = json!({
        "format": fmt,
        "max_dim": max,
        "original_dims": info.original.map(|(w,h)| json!([w,h])).unwrap_or(Value::Null),
        "target_dims": info.target.map(|(w,h)| json!([w,h])).unwrap_or(Value::Null),
        "scale": info.scale,
        "would_resize": info.would_resize,
        "did_resize": info.did_resize,
    });
    if let Some(note) = resize_note(&info) {
        meta["note"] = Value::String(note);
    }
    let encoded = if as_data_uri {
        let mime = mime_for_format(detect_format(&out_bytes));
        to_data_uri(&out_bytes, mime)
    } else {
        encode_base64(&out_bytes)
    };
    Ok((encoded, meta))
}

// ---------------------------------------------------------------------------
// Annotation — numeric labels + bounding boxes (deterministic, non-destructive)
// ---------------------------------------------------------------------------

/// Annotation legend entry for one candidate.
#[derive(Debug, Clone, Serialize)]
pub struct AnnotationEntry {
    pub id: u32,
    pub label: String,
    pub tag: String,
    pub role: String,
    pub name: String,
    pub rect: Rect,
    pub selector: String,
}

/// Build deterministic legend JSON for a candidate set, ordered by id ascending.
pub fn annotation_legend(cands: &Candidates) -> Value {
    let entries: Vec<AnnotationEntry> = cands
        .iter()
        .map(|c| AnnotationEntry {
            id: c.id,
            label: c.label.clone(),
            tag: c.tag.clone(),
            role: c.role.clone(),
            name: c.name.clone(),
            rect: c.rect,
            selector: c.selector.clone(),
        })
        .collect();
    json!(entries)
}

/// Annotate a screenshot for visual candidate selection (vision mode).
///
/// Without the `image` crate, this is a **non-destructive JSON overlay**:
/// - Original image bytes are returned unchanged (clone).
/// - `meta` contains `annotations` (legend + rects) and `annotated=false`.
///   The caller should forward both `image` (base64/data URI of original bytes)
///   and `annotations` JSON to the vision model (e.g. as text context or as
///   sidecar). Vision prompts should reference candidates as "label 1 at (x,y)
///   — 'Submit'" using the deterministic legend.
///
/// With the `image` crate (future), this would rasterize numbered boxes
/// onto a copy of the image (red border, yellow label background, numeric
/// "1","2"…), returning the annotated JPEG bytes with `annotated=true`.
///
/// Never mutates the original image buffer in place; always copies.
pub fn annotate_screenshot(
    bytes: &[u8],
    cands: &Candidates,
    respect_max_dim: bool,
) -> Result<(Vec<u8>, Value)> {
    let legend = annotation_legend(cands);
    let dims = detect_dimensions(bytes);
    let fmt = detect_format(bytes);

    // Candidate rects as viewport-relative JSON for the vision model.
    // Hints already provide viewport-relative rects (getBoundingClientRect).
    // For grim captures, rects would need to be translated relative to capture geometry;
    // we expose both global and viewport-relative as stored.

    let max_dim = if respect_max_dim {
        max_image_dim_from_env()
    } else {
        u32::MAX
    };
    let (_, resize_info) = maybe_resize(bytes.to_vec(), max_dim);

    let meta = json!({
        "annotated": false,
        "annotation_mode": "json_legend",
        "format": fmt,
        "dimensions": dims.map(|(w,h)| json!([w,h])).unwrap_or(Value::Null),
        "candidate_count": cands.len(),
        "vision_budget": crate::decider::candidates::VISION_MAX_OPTIONS,
        "legend": legend,
        "note": "pure Rust: no image crate — returning original bytes + JSON legend (deterministic, visually clear via legend text). To enable rasterized boxes, add `image` crate with features jpeg,png and implement draw on copy.",
        "resize": {
            "would_resize": resize_info.would_resize,
            "did_resize": resize_info.did_resize,
            "scale": resize_info.scale,
            "original": resize_info.original.map(|(w,h)| json!([w,h])).unwrap_or(Value::Null),
            "target": resize_info.target.map(|(w,h)| json!([w,h])).unwrap_or(Value::Null)
        }
    });

    // Return a copy of original bytes (non-destructive).
    Ok((bytes.to_vec(), meta))
}

/// Helper: annotate and immediately encode for Decider vision (base64/data URI).
/// Returns `(encoded_image, meta_with_legend)`.
pub fn annotate_and_encode(
    bytes: Vec<u8>,
    cands: &Candidates,
    as_data_uri: bool,
    respect_max_dim: bool,
) -> Result<(String, Value)> {
    let (annotated_bytes, mut annotate_meta) = annotate_screenshot(&bytes, cands, respect_max_dim)?;
    // Re-apply max-dim after annotation (bytes still original).
    let max = if respect_max_dim {
        max_image_dim_from_env()
    } else {
        u32::MAX
    };
    let (final_bytes, resize_info) = if respect_max_dim {
        maybe_resize(annotated_bytes, max)
    } else {
        (annotated_bytes, ResizeInfo {
            original: detect_dimensions(&bytes),
            target: detect_dimensions(&bytes),
            did_resize: false,
            would_resize: false,
            scale: 1.0,
            max_dim: max,
        })
    };
    if resize_info.would_resize && !resize_info.did_resize {
        annotate_meta["resize"]["would_resize"] = Value::Bool(true);
        annotate_meta["resize"]["note"] = Value::String(format!(
            "would resize {}x{} -> {}x{} but no image crate — forwarded original",
            resize_info.original.map(|(w,_)| w).unwrap_or(0),
            resize_info.original.map(|(_,h)| h).unwrap_or(0),
            resize_info.target.map(|(w,_)| w).unwrap_or(0),
            resize_info.target.map(|(_,h)| h).unwrap_or(0),
        ));
    }
    let encoded = if as_data_uri {
        to_data_uri_auto(&final_bytes)
    } else {
        encode_base64(&final_bytes)
    };
    Ok((encoded, annotate_meta))
}

// ---------------------------------------------------------------------------
// Convenience pipelines for hint/candidate regions
// ---------------------------------------------------------------------------

/// Capture a candidate's region via grim and encode for Decider (text or vision).
/// Uses `ImageSource::CandidateRect`.
pub fn pipeline_for_candidate(
    candidate: &crate::decider::candidates::Candidate,
    max_dim: Option<u32>,
    as_data_uri: bool,
) -> Result<(String, Value)> {
    let source = ImageSource::CandidateRect(candidate.rect);
    pipeline(&source, max_dim, as_data_uri, 0.0)
}

/// Capture browser viewport and annotate with the full candidate set for vision
/// (10-per-page). Encodes as data URI by default for vision models.
pub fn vision_pipeline(
    cands: &Candidates,
    source: &ImageSource,
    max_dim: Option<u32>,
) -> Result<(String, Value, Value)> {
    // source is typically Browser or Monitor. Capture once, annotate, chunk if needed.
    let (bytes, cap_meta) = capture_with_source(source, 0.0)?;
    let total = cands.len();
    if total <= crate::decider::candidates::VISION_MAX_OPTIONS {
        let (encoded, ann_meta) = annotate_and_encode(bytes, cands, true, true)?;
        let mut meta = cap_meta;
        meta["vision"] = ann_meta;
        meta["pipeline_for"] = json!({"vision_chunks": 1, "total_candidates": total});
        // also expose max_dim handling
        let max = max_dim.unwrap_or_else(max_image_dim_from_env);
        meta["max_dim"] = json!(max);
        return Ok((encoded, meta.clone(), meta));
    }
    // Over budget: hierarchical — group into 10s, announce grouping in meta.
    // Caller should iterate groups with separate screenshots per group? For now
    // we reuse the same screenshot annotated per chunk and return first chunk's
    // encoding; full hierarchical orchestration is done by the Decider client.
    let groups = cands.chunks_for_vision();
    let first_group = Candidates::from_vec(groups[0].clone());
    let (encoded, ann_meta) = annotate_and_encode(bytes, &first_group, true, true)?;
    let mut meta = cap_meta;
    meta["vision"] = ann_meta;
    meta["vision"]["hierarchical_groups"] = json!(groups.len());
    meta["vision"]["group_sizes"] = json!(groups.iter().map(|g| g.len()).collect::<Vec<_>>());
    meta["vision"]["note"] = Value::String(format!(
        "vision over budget: {total} candidates -> {} groups of <=10; hierarchical selection required (deterministic filtering then group-winners -> final set -> final Decider call)",
        groups.len()
    ));
    Ok((encoded, meta.clone(), meta))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_detection() {
        let png_magic = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0];
        assert_eq!(detect_format(&png_magic), "png");
        let jpeg_magic = [0xFF, 0xD8, 0xFF, 0xE0];
        assert_eq!(detect_format(&jpeg_magic), "jpeg");
        assert_eq!(detect_format(b"hello"), "unknown");
    }

    #[test]
    fn base64_and_data_uri() {
        let bytes = b"hello world";
        let b64 = encode_base64(bytes);
        assert_eq!(b64, "aGVsbG8gd29ybGQ=");
        let uri = to_data_uri(bytes, "image/jpeg");
        assert!(uri.starts_with("data:image/jpeg;base64,"));
        let decoded = decode_image_field(&uri).unwrap();
        assert_eq!(decoded, bytes);
        let decoded2 = decode_image_field(&b64).unwrap();
        assert_eq!(decoded2, bytes);
    }

    #[test]
    fn data_uri_auto() {
        // tiny png-like header for detection
        let mut png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        png.extend(vec![0u8; 100]);
        let uri = to_data_uri_auto(&png);
        assert!(uri.contains("image/png"));
    }

    #[test]
    fn detect_dims_png() {
        // Minimal PNG with IHDR width=2 height=3
        let mut png: Vec<u8> = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        png.extend([0, 0, 0, 13]); // chunk len
        png.extend(*b"IHDR");
        png.extend([0, 0, 0, 2]); // w=2
        png.extend([0, 0, 0, 3]); // h=3
        png.extend([8, 2, 0, 0, 0]); // rest
        png.extend([0, 0, 0, 0]); // crc placeholder
        let dims = detect_dimensions(&png);
        assert_eq!(dims, Some((2, 3)));
    }

    #[test]
    fn needs_resize_check() {
        assert!(needs_resize(Some((2000, 1000)), 1280));
        assert!(!needs_resize(Some((1000, 800)), 1280));
        assert!(!needs_resize(None, 1280));
    }

    #[test]
    fn maybe_resize_noop() {
        // unknown dims -> no resize
        let bytes = b"not an image".to_vec();
        let (out, info) = maybe_resize(bytes.clone(), 1280);
        assert_eq!(out, bytes);
        assert!(!info.would_resize);
    }

    #[test]
    fn annotate_is_nondestructive() {
        let bytes = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0];
        let mut v = Vec::new();
        for i in 1..=3u32 {
            v.push(crate::decider::candidates::Candidate {
                id: i,
                label: format!("L{}", i),
                tag: "button".into(),
                role: "button".into(),
                name: format!("Btn {}", i),
                text: format!("Btn {}", i),
                rect: Rect::new(i as i32 * 10, i as i32 * 10, 80, 30),
                visible: true,
                enabled: true,
                selector: format!("#b{}", i),
                target_id: None,
                url: None,
                title: None,
            });
        }
        let cands = Candidates::from_vec(v);
        let (out, meta) = annotate_screenshot(&bytes, &cands, false).unwrap();
        assert_eq!(out, bytes); // nondestructive
        assert_eq!(meta["annotated"], json!(false));
        assert_eq!(meta["candidate_count"], json!(3));
        assert!(meta["legend"].as_array().unwrap().len() == 3);
        // first legend id is 1, stable
        assert_eq!(meta["legend"][0]["id"], json!(1));
    }

    #[test]
    fn encode_for_decider() {
        let bytes = b"fake-jpeg".to_vec();
        let (enc, meta) = super::encode_for_decider(bytes, Some(1280), false).unwrap();
        assert!(!enc.is_empty());
        assert!(meta.get("max_dim").is_some());
    }

    #[test]
    fn max_dim_env_default() {
        assert_eq!(max_image_dim_from_env(), 1280);
        std::env::set_var("DECIDER_MAX_IMAGE_DIM", "640");
        assert_eq!(max_image_dim_from_env(), 640);
        std::env::remove_var("DECIDER_MAX_IMAGE_DIM");
        assert_eq!(max_image_dim_from_env(), 1280);
    }
}

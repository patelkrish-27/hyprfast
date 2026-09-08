import re

with open("src/browser_runtime/executor.rs", "r") as f:
    content = f.read()

# Add imports
imports = """use super::vision::{VisualTarget, VisionEngine};
use super::recovery::{RecoveryEngine, RecoveryOptions, RecoveryOutcome};

#[derive(Debug, Clone)]
pub enum ResolvedTarget {
    Element(ElementRef),
    Visual(VisualTarget),
}
"""
content = content.replace("use super::action_journal::ActionJournal;", "use super::action_journal::ActionJournal;\n" + imports)

# Update resolve_for_step return type
content = content.replace("RuntimeResult<Option<ElementRef>>", "RuntimeResult<Option<ResolvedTarget>>")
content = content.replace("let resolution_result: RuntimeResult<Option<ElementRef>> = match timeout(", "let resolution_result: RuntimeResult<Option<ResolvedTarget>> = match timeout(")
content = content.replace("let resolved_ref: Option<ElementRef> = match resolution_result {", "let resolved_ref: Option<ResolvedTarget> = match resolution_result {")
content = content.replace("resolved: &Option<ElementRef>,", "resolved: &Option<ResolvedTarget>,")

# Update element_index.resolve calls
resolve_match_pattern = r"""match self\.element_index\.resolve\(&req\) \{
\s*Ok\(r\) => Ok\(Some\(r\)\),
\s*Err\(RuntimeError::ResolutionFailed\(\_\)\) => \{.*?\s*Ok\(None\)\s*\}
\s*Err\(e\) => Err\(e\),
\s*\}"""

def replace_resolve(match):
    return """match self.element_index.resolve(&req) {
                        Ok(r) => Ok(Some(ResolvedTarget::Element(r))),
                        Err(RuntimeError::ResolutionFailed(_)) => {
                            // Try recovery
                            let recovery_engine = RecoveryEngine::new(
                                self.element_index.clone(),
                                self.frame_manager.clone(),
                                self.dom_state.clone(),
                            );
                            
                            // Check if it's a visual context (e.g. canvas) based on selector or tag
                            let is_visual = req.selector.as_deref().unwrap_or("").contains("canvas") || req.tag_name.as_deref().unwrap_or("") == "canvas";
                            
                            let opts = RecoveryOptions {
                                allow_vision: true,
                                is_visual_context: is_visual,
                                pierce_shadow: true,
                            };
                            let rec_res = recovery_engine.recover(None, &req, opts);
                            match rec_res.outcome {
                                RecoveryOutcome::Recovered(r) => Ok(Some(ResolvedTarget::Element(r))),
                                RecoveryOutcome::VisionRequired { detail } => {
                                    let vt = VisionEngine::locate_visually(&self.runtime, session_id, target_id, &detail).await?;
                                    Ok(Some(ResolvedTarget::Visual(vt)))
                                }
                                _ => Ok(None)
                            }
                        }
                        Err(e) => Err(e),
                    }"""

content = re.sub(r"""match self\.element_index\.resolve\(&req\) \{\s*Ok\(r\) => Ok\(Some\(r\)\),\s*Err\(RuntimeError::ResolutionFailed\(\_\)\) => [^}]*?Ok\(None\)[^}]*?\}\s*Err\(e\) => Err\(e\),\s*\}""", replace_resolve, content)

content = re.sub(r"""match self\.element_index\.resolve\(&req\) \{\s*Ok\(r\) => Ok\(Some\(r\)\),\s*Err\(RuntimeError::ResolutionFailed\(\_\)\) => Ok\(None\),\s*Err\(e\) => Err\(e\),\s*\}""", replace_resolve, content)

# dispatch_step handles VisualTarget
dispatch_click_pattern = r"""if let Some\(ref r\) = resolved \{
\s*// Use DOM\.resolveNode \+ click via callFunctionOn \(preferred, no JS interpolation\)
\s*// For simplicity, use Runtime\.evaluate with selector derived from ref's selector
\s*let js = format!\(
\s*"let el = document\.querySelector\('\{\}'\); if \(el\) \{\{ el\.click\(\); true \}\} else \{\{ false \}\}",
\s*r\.selector\.replace\("'", "\\\\'"\)
\s*\);
\s*self\.runtime\.call\(eff, "Runtime\.evaluate", serde_json::json!\(\{"expression": js\}\)\)\.await\?
\s*\} else \{"""

new_dispatch_click = """if let Some(ResolvedTarget::Element(ref r)) = resolved {
                    let js = format!(
                        "let el = document.querySelector('{}'); if (el) {{ el.click(); true }} else {{ false }}",
                        r.selector.replace("'", "\\'")
                    );
                    self.runtime.call(eff, "Runtime.evaluate", serde_json::json!({"expression": js})).await?
                } else if let Some(ResolvedTarget::Visual(ref vt)) = resolved {
                    // Dispatch synthetic mouse events at vt.point
                    let (x, y) = vt.point;
                    self.runtime.call(eff, "Input.dispatchMouseEvent", serde_json::json!({
                        "type": "mousePressed", "x": x, "y": y, "button": "left", "clickCount": 1
                    })).await?;
                    self.runtime.call(eff, "Input.dispatchMouseEvent", serde_json::json!({
                        "type": "mouseReleased", "x": x, "y": y, "button": "left", "clickCount": 1
                    })).await?
                } else {"""
content = re.sub(dispatch_click_pattern, new_dispatch_click, content)


# pre_dispatch_hook uses ResolvedTarget
content = content.replace("if let Some(r) = resolved_ref.as_ref() {", "if let Some(ResolvedTarget::Element(r)) = resolved_ref.as_ref() {")
# we must also replace it in tests if applicable.

# In tests:
content = content.replace("let resolved: Option<ElementRef> = None;", "let resolved: Option<ResolvedTarget> = None;")

with open("src/browser_runtime/executor.rs", "w") as f:
    f.write(content)


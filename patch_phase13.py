import re

with open("src/browser_runtime/mod.rs", "r") as f:
    mod_content = f.read()

if "pub mod vision;" not in mod_content:
    mod_content = mod_content.replace("pub mod wait;", "pub mod wait;\npub mod vision;")
    mod_content = mod_content.replace("pub use wait::", "pub use vision::*;\npub use wait::")
    with open("src/browser_runtime/mod.rs", "w") as f:
        f.write(mod_content)

with open("src/browser_runtime/executor.rs", "r") as f:
    content = f.read()

# Add imports
if "ResolvedTarget" not in content:
    imports = """use super::vision::{VisualTarget, VisionEngine};
use super::recovery::{RecoveryEngine, RecoveryOptions, RecoveryOutcome};

#[derive(Debug, Clone)]
pub enum ResolvedTarget {
    Element(ElementRef),
    Visual(VisualTarget),
}
"""
    content = content.replace("use super::action_journal::ActionJournal;", "use super::action_journal::ActionJournal;\n" + imports)

# Rename _session_id to session_id
content = content.replace("    async fn resolve_for_step(\n        &self,\n        step: &PlanStep,\n        target_id: Option<&str>,\n        _session_id: Option<&str>,\n    ) -> RuntimeResult<Option<ElementRef>>", "    async fn resolve_for_step(\n        &self,\n        step: &PlanStep,\n        target_id: Option<&str>,\n        session_id: Option<&str>,\n    ) -> RuntimeResult<Option<ResolvedTarget>>")

content = content.replace("let resolution_result: RuntimeResult<Option<ElementRef>> = match timeout(", "let resolution_result: RuntimeResult<Option<ResolvedTarget>> = match timeout(")
content = content.replace("let resolved_ref: Option<ElementRef> = match resolution_result {", "let resolved_ref: Option<ResolvedTarget> = match resolution_result {")
content = content.replace("resolved: &Option<ElementRef>,", "resolved: &Option<ResolvedTarget>,")

# Replace element_index.resolve calls
resolve_repl = """match self.element_index.resolve(&req) {
                        Ok(r) => Ok(Some(ResolvedTarget::Element(r))),
                        Err(RuntimeError::ResolutionFailed(_)) => {
                            let recovery_engine = RecoveryEngine::new(
                                self.element_index.clone(),
                                self.frame_manager.clone(),
                                self.dom_state.clone(),
                            );
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

content = re.sub(r"""match self\.element_index\.resolve\(&req\) \{\s*Ok\(r\) => Ok\(Some\(r\)\),\s*Err\(RuntimeError::ResolutionFailed\(\_\)\) => \{[^\}]*?Ok\(None\)[^\}]*?\}\s*Err\(e\) => Err\(e\),\s*\}""", resolve_repl, content)
content = re.sub(r"""match self\.element_index\.resolve\(&req\) \{\s*Ok\(r\) => Ok\(Some\(r\)\),\s*Err\(RuntimeError::ResolutionFailed\(\_\)\) => Ok\(None\),\s*Err\(e\) => Err\(e\),\s*\}""", resolve_repl, content)

# Fix references to resolved_ref / r in pre-dispatch checks
content = content.replace("if let Some(r) = resolved_ref.as_ref() {", "if let Some(ResolvedTarget::Element(r)) = resolved_ref.as_ref() {")

# Fix tests
content = content.replace("let resolved: Option<ElementRef> = None;", "let resolved: Option<ResolvedTarget> = None;")

# Fix dispatch_step
new_dispatch_click = """if let Some(ResolvedTarget::Element(ref r)) = resolved {
                    let js = format!(
                        "let el = document.querySelector('{}'); if (el) {{ el.click(); true }} else {{ false }}",
                        r.selector.replace("'", "\\'")
                    );
                    self.runtime.call(eff, "Runtime.evaluate", serde_json::json!({"expression": js})).await?
                } else if let Some(ResolvedTarget::Visual(ref vt)) = resolved {
                    let (x, y) = vt.point;
                    self.runtime.call(eff, "Input.dispatchMouseEvent", serde_json::json!({
                        "type": "mousePressed", "x": x, "y": y, "button": "left", "clickCount": 1
                    })).await?;
                    self.runtime.call(eff, "Input.dispatchMouseEvent", serde_json::json!({
                        "type": "mouseReleased", "x": x, "y": y, "button": "left", "clickCount": 1
                    })).await?
                } else {"""
content = re.sub(r"""if let Some\(ref r\) = resolved \{\s*// Use DOM\.resolveNode[^\}]*?self\.runtime\.call[^\}]*?\}\s*else\s*\{""", new_dispatch_click, content)

content = content.replace("if let Some(ref r) = resolved {", "if let Some(ResolvedTarget::Element(ref r)) = resolved {")

with open("src/browser_runtime/executor.rs", "w") as f:
    f.write(content)

print("Patched.")

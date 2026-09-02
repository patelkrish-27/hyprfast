//! Port of deepLocator.ts + selectorResolver.ts + locatorInvocation.ts
use anyhow::Result;
use serde_json::Value;

pub fn build_locator_invocation(name: &str, args: &[Value]) -> String {
    format!("globalThis.__stagehandLocatorScripts['{}']({})", name, args.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(","))
}
pub fn deep_locator_through_iframes(_selector: &str) -> Result<Vec<String>> { Ok(vec![_selector.to_string()]) }
pub fn resolve_locator_target(_page_id: &str, _selector: &str) -> Result<Option<String>> { Ok(None) }

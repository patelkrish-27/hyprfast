//! Hybrid snapshot — port of packages/extension/understudy/a11y/snapshot/capture.ts
//! Simplified for hyprfast: uses CDP Accessibility.getFullAXTree + DOM.getDocument
//! Hybrid = AX tree trimmed + xpath map + frame ordinals (stagehand encodes as "0-1234")
//! Hyprfast already has browser::snapshot() via cdp; this upgrades it to stagehand fidelity

use anyhow::{Result, bail};
use serde_json::{Value, json};
use crate::cdp;

/// Stagehand HybridSnapshot shape (subset)
#[derive(Debug, Clone)]
pub struct HybridSnapshot {
    pub combined_tree: String, // textual tree with [frame-backendId] markers
    pub combined_xpath_map: Value, // encId -> xpath
    pub combined_url_map: Value,
    pub raw_ax_nodes: usize,
    pub via: String,
}

/// Capture hybrid snapshot using CDP Accessibility + optional frame walk
/// Mirrors captureHybridSnapshot() fast-path
pub fn capture_hybrid() -> Result<HybridSnapshot> {
    let ws = cdp::get_ws_url(None)?;
    // Ensure domains
    let _ = cdp::cdp_call(&ws, "DOM.enable", json!({}));
    let _ = cdp::cdp_call(&ws, "Accessibility.enable", json!({}));

    // 1. Try Accessibility.getFullAXTree (stagehand primary)
    let ax_res = cdp::cdp_call(&ws, "Accessibility.getFullAXTree", json!({}));
    if let Ok(val) = ax_res {
        if let Some(nodes) = val.get("nodes").and_then(|v| v.as_array()) {
            if !nodes.is_empty() {
                let (tree, xpath_map) = build_tree_from_ax(nodes, 0);
                if !tree.is_empty() {
                    return Ok(HybridSnapshot {
                        combined_tree: tree,
                        combined_xpath_map: xpath_map,
                        combined_url_map: json!({}),
                        raw_ax_nodes: nodes.len(),
                        via: "Accessibility.getFullAXTree".into(),
                    });
                }
            }
        }
    }
    // 2. Fallback: JS hybrid tree builder (mirrors stagehand treeFormatUtils)
    let js_tree = snapshot_via_js(&ws)?;
    Ok(HybridSnapshot {
        combined_tree: js_tree.get("tree").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        combined_xpath_map: js_tree.get("xpathMap").cloned().unwrap_or(json!({})),
        combined_url_map: json!({}),
        raw_ax_nodes: 0,
        via: "Runtime.evaluate hybrid".into(),
    })
}

fn build_tree_from_ax(nodes: &[Value], frame_ordinal: i32) -> (String, Value) {
    // Stagehand encodes as "[frameOrdinal-backendId]" + role + name + xpath
    // Do similar to browser/mod.rs build_snapshot_from_ax but with tree string
    let mut lines = Vec::new();
    let mut xpath_map = serde_json::Map::new();
    for n in nodes.iter().take(300) {
        let ignored = n.get("ignored").and_then(|v| v.as_bool()).unwrap_or(false);
        if ignored { continue; }
        let role = n.get("role").and_then(|r| r.get("value")).and_then(|v| v.as_str()).unwrap_or("");
        let name = n.get("name").and_then(|r| r.get("value")).and_then(|v| v.as_str()).unwrap_or("");
        let backend = n.get("backendDOMNodeId").and_then(|v| v.as_i64()).unwrap_or(0);
        if backend==0 && name.is_empty() && role.is_empty() { continue; }
        let node_id = n.get("nodeId").and_then(|v| v.as_str()).unwrap_or("");
        // Build id like Stagehand
        let enc = format!("{}-{}", frame_ordinal, backend);
        let xpath = n.get("properties").and_then(|p| p.as_array())
            .and_then(|arr| arr.iter().find(|x| x.get("name").and_then(|v| v.as_str())==Some("xpath")))
            .and_then(|x| x.get("value").and_then(|v| v.get("value")).and_then(|v| v.as_str()))
            .unwrap_or("").to_string();
        if !xpath.is_empty() { xpath_map.insert(enc.clone(), Value::String(xpath)); }
        // Depth heuristic from childIds? Stagehand uses outline with indent — do flat with markers
        let line = format!("[{}] {} '{}' {}", enc, role, name.chars().take(120).collect::<String>(), node_id);
        lines.push(line);
        if lines.len()>=150 { break; }
    }
    // Trim for token efficiency — stagehand does hybrid trimming to ~8192 tokens
    let tree = trim_tree(&lines.join("\n"), 8000);
    (tree, Value::Object(xpath_map))
}

fn trim_tree(s: &str, max_chars: usize) -> String {
    if s.len() <= max_chars { return s.to_string(); }
    // Stagehand trims keep top + bottom, prioritize interactive — simple truncate
    let mut out = s.chars().take(max_chars).collect::<String>();
    out.push_str("\n...[trimmed for token efficiency - Stagehand hybrid trimming]...");
    out
}

fn snapshot_via_js(ws: &str) -> Result<Value> {
    // Mirrors stagehand's injected script but simpler — produce tree + xpathMap
    let js = r#"
(() => {
  const MAX = 200;
  const treeLines = [];
  const xpathMap = {};
  let count=0;
  const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_ELEMENT);
  let n = walker.currentNode;
  const getXpath = (el) => {
    const parts=[];
    let cur=el;
    while(cur && cur!==document.body && cur!==document.documentElement && parts.length<6){
      let idx=1; let sib=cur.previousElementSibling;
      while(sib){ if(sib.tagName===cur.tagName) idx++; sib=sib.previousElementSibling; }
      parts.unshift(cur.tagName.toLowerCase()+"["+idx+"]");
      cur=cur.parentElement;
    }
    return "//"+parts.join("/");
  };
  while(n && count<MAX){
    const el=n;
    const tag=el.tagName.toLowerCase();
    const role=el.getAttribute('role')||({'a':'link','button':'button','input':'textbox','select':'combobox','textarea':'textbox'}[tag]||tag);
    const name=(el.getAttribute('aria-label')||el.innerText||el.value||el.placeholder||'').trim().slice(0,100);
    const rect=el.getBoundingClientRect();
    const visible=rect.width>0 && rect.height>0 && getComputedStyle(el).visibility!=='hidden';
    if(visible || name){
      // approximate backendId via count (stagehand uses CDP backendNodeId; we fake ordinal 0)
      const enc = "0-"+(10000+count);
      const xpath = getXpath(el);
      xpathMap[enc]=xpath;
      treeLines.push("["+enc+"] "+role+" '"+name+"' "+tag+" ("+Math.round(rect.x)+","+Math.round(rect.y)+")");
      count++;
    }
    n=walker.nextNode();
  }
  return {tree: treeLines.join("\n"), xpathMap};
})()
"#;
    let params = json!({"expression": js, "returnByValue": true, "awaitPromise": false});
    let res = cdp::cdp_call(ws, "Runtime.evaluate", params)?;
    let val = res.get("result").and_then(|r| r.get("value")).cloned().unwrap_or(Value::Null);
    Ok(val)
}

/// For actHandlerUtils — resolve encId -> backend
pub fn parse_enc_id(enc: &str) -> Option<(i32,i64)> {
    let mut sp = enc.splitn(2, '-');
    let ord: i32 = sp.next()?.parse().ok()?;
    let backend: i64 = sp.next()?.parse().ok()?;
    Some((ord, backend))
}

/// Diff helper for two-step dropdown detection
pub fn diff_trees(a: &str, b: &str) -> String {
    // Simple: return added lines — stagehand uses diffCombinedTrees
    let set_a: std::collections::HashSet<&str> = a.lines().collect();
    let added: Vec<&str> = b.lines().filter(|l| !set_a.contains(l)).collect();
    if added.is_empty() { b.to_string() } else { added.join("\n") }
}

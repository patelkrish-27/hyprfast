//! Port of packages/extension/understudy/a11y/snapshot/xpathUtils.ts + dom/a11yScripts/index.ts nodeToAbsoluteXPath

use anyhow::Result;
use serde_json::json;

const NODE_TO_XPATH_JS: &str = r#"
function() {
  const sibIndex = (n) => {
    if (!n || !n.parentNode) return 1;
    let i=1; const key = `${n.nodeType}:${(n.nodeName||"").toLowerCase()}`;
    for(let p=n.previousSibling; p; p=p.previousSibling){
      const k=`${p.nodeType}:${(p.nodeName||"").toLowerCase()}`;
      if(k===key) i++;
    }
    return i;
  };
  const step = (n) => {
    if(!n) return "";
    if(n.nodeType===9) return "";
    if(n.nodeType===11) return "//";
    if(n.nodeType===3) return `text()[${sibIndex(n)}]`;
    if(n.nodeType===8) return `comment()[${sibIndex(n)}]`;
    const tag=(n.nodeName||"").toLowerCase();
    const name=tag.includes(":")?`*[name()='${tag}']`:tag;
    return `${name}[${sibIndex(n)}]`;
  };
  let cur=this; const parts=[];
  while(cur){
    if(cur.nodeType===11){ parts.push("//"); cur=cur.host||null; continue; }
    const s=step(cur); if(s) parts.push(s); cur=cur.parentNode;
  }
  parts.reverse();
  let out="";
  for(const part of parts){
    if(part==="\/\/"){ out=out ? (out.endsWith("/")?`${out}/`:`${out}//`):"//"; }
    else { out=out ? (out.endsWith("/")?`${out}${part}`:`${out}/${part}`):`/${part}`; }
  }
  return out||"/";
}
"#;

/// Build absolute xpath from iframe chain + leaf — mirrors buildAbsoluteXPathFromChain
pub fn build_absolute_xpath_from_chain(chain: &[(String, u32)], leaf_backend: u32) -> Result<Option<String>> {
    let ws = crate::cdp::get_ws_url(None)?;
    let mut prefix = String::new();
    for (_, backend) in chain {
        if let Some(xp) = absolute_xpath_for_backend_node_inner(&ws, *backend)? {
            prefix = if prefix.is_empty() { normalize_xpath(&xp) } else { prefix_xpath(&prefix, &xp) };
        }
    }
    let leaf = absolute_xpath_for_backend_node_inner(&ws, leaf_backend)?;
    Ok(match leaf {
        Some(l) => Some(if prefix.is_empty() { normalize_xpath(&l) } else { prefix_xpath(&prefix, &l) }),
        None => Some(if prefix.is_empty() { "/".to_string() } else { prefix }),
    })
}

fn absolute_xpath_for_backend_node_inner(ws: &str, backend: u32) -> Result<Option<String>> {
    let resolved = crate::cdp::cdp_call(ws, "DOM.resolveNode", json!({"backendNodeId": backend}))?;
    let oid = resolved.get("object").and_then(|o| o.get("objectId")).and_then(|v| v.as_str());
    let Some(oid) = oid else { return Ok(None) };
    let r = crate::cdp::cdp_call(ws, "Runtime.callFunctionOn", json!({"objectId": oid, "functionDeclaration": NODE_TO_XPATH_JS, "returnByValue": true}))?;
    let _ = crate::cdp::cdp_call(ws, "Runtime.releaseObject", json!({"objectId": oid}));
    Ok(r.get("result").and_then(|x| x.get("value")).and_then(|v| v.as_str()).map(|s| s.to_string()))
}

pub fn absolute_xpath_for_backend_node(backend: u32) -> Result<Option<String>> {
    let ws = crate::cdp::get_ws_url(None)?;
    absolute_xpath_for_backend_node_inner(&ws, backend)
}

pub fn prefix_xpath(parent_abs: &str, child: &str) -> String {
    let p = if parent_abs == "/" { "" } else { parent_abs.trim_end_matches('/') };
    if child.is_empty() || child == "/" { return if p.is_empty() { "/".to_string() } else { p.to_string() } }
    if child.starts_with("//") { return if p.is_empty() { format!("//{}", &child[2..]) } else { format!("{}//{}", p, &child[2..]) } }
    let c = child.trim_start_matches('/');
    if p.is_empty() { format!("/{}", c) } else { format!("{}/{}", p, c) }
}

pub fn normalize_xpath(x: &str) -> String {
    if x.is_empty() { return String::new() }
    let mut s = x.trim().trim_start_matches(|c| c=='x'||c=='X').to_string();
    // handle xpath= prefix case
    if s.to_lowercase().starts_with("xpath=") { s = s[5..].to_string(); }
    s = s.trim().to_string();
    if !s.starts_with('/') { s = format!("/{}", s); }
    if s.len() > 1 && s.ends_with('/') { s.pop(); }
    s
}

pub fn build_child_xpath_segments(kids: &[serde_json::Value]) -> Vec<String> {
    let mut segs = vec![];
    let mut ctr: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for child in kids {
        let tag = child.get("nodeName").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
        let nt = child.get("nodeType").and_then(|v| v.as_u64()).unwrap_or(1);
        let key = format!("{}:{}", nt, tag);
        let idx = { let e = ctr.entry(key.clone()).or_insert(0); *e+=1; *e };
        if nt==3 { segs.push(format!("text()[{}]", idx)); }
        else if nt==8 { segs.push(format!("comment()[{}]", idx)); }
        else { segs.push(if tag.contains(':') { format!("*[name()='{}'][{}]", tag, idx) } else { format!("{}[{}]", tag, idx) }); }
    }
    segs
}

pub fn join_xpath(base: &str, step: &str) -> String {
    if step=="//" { if base.is_empty() || base=="/" { return "//".to_string() } else { return if base.ends_with('/') { format!("{}/", base) } else { format!("{}//", base) } } }
    if base.is_empty() || base=="/" { return if step.is_empty() { "/".to_string() } else { format!("/{}", step) } }
    if base.ends_with("//") { return format!("{}{}", base, step) }
    if step.is_empty() { return base.to_string() }
    format!("{}/{}", base, step)
}

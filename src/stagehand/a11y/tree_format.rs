//! Port of packages/extension/understudy/a11y/snapshot/treeFormatUtils.ts
//! Pure formatting helpers — tested independently in Stagehand.

pub fn format_tree_line(role: &str, name: &str, enc_id: &str, level: usize, selected: bool, checked: bool) -> String {
    let indent = "  ".repeat(level);
    let flags = format!("{}{}", if selected { " [selected]" } else { "" }, if checked { " [checked]" } else { "" });
    let name_part = if name.is_empty() { String::new() } else { format!(": {}", clean_text(name)) };
    format!("{}[{}] {}{}{}", indent, enc_id, role, name_part, flags)
}

pub fn inject_subtrees(root_outline: &str, id_to_tree: &std::collections::HashMap<String,String>) -> String {
    let mut out: Vec<String> = vec![];
    let mut visited = std::collections::HashSet::new();
    let lines: Vec<String> = root_outline.split('\n').map(|l| l.to_string()).collect();
    // iterative stack like TS
    let mut stack: Vec<(Vec<String>, usize)> = vec![(lines, 0)];
    while let Some((ls, i)) = stack.last_mut() {
        if *i >= ls.len() { stack.pop(); continue; }
        let raw = ls[*i].clone(); *i+=1;
        let indent_len = raw.chars().take_while(|c| *c==' ').count();
        let indent = &raw[..indent_len];
        let content = &raw[indent_len..];
        out.push(raw.clone());
        if let Some(br) = content.find(']') {
            if content.starts_with('[') {
                let enc = &content[1..br];
                if let Some(child) = id_to_tree.get(enc) {
                    if !visited.contains(enc) {
                        visited.insert(enc.to_string());
                        let injected = inject_subtrees(child, id_to_tree);
                        let block = indent_block(injected.trim_end(), &(indent.to_string()+"  "));
                        out.push(block);
                    }
                }
            }
        }
    }
    out.join("\n")
}

pub fn indent_block(block: &str, indent: &str) -> String {
    if block.is_empty() { return String::new() }
    block.split('\n').map(|l| format!("{}{}", indent, l)).collect::<Vec<_>>().join("\n")
}

pub fn diff_combined_trees(prev: &str, next: &str) -> String {
    let prev_set: std::collections::HashSet<String> = prev.split('\n').map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
    let next_lines: Vec<&str> = next.split('\n').collect();
    let mut added: Vec<String> = vec![];
    for line in next_lines { let core=line.trim(); if core.is_empty() { continue; } if !prev_set.contains(core) { added.push(line.to_string()); } }
    if added.is_empty() { return String::new() }
    let min_indent = added.iter().filter(|l| !l.trim().is_empty()).map(|l| l.chars().take_while(|c| *c==' ').count()).min().unwrap_or(0);
    added.into_iter().map(|l| if l.len()>=min_indent { l[min_indent..].to_string() } else { l }).collect::<Vec<_>>().join("\n")
}

pub fn clean_text(input: &str) -> String {
    const PUA_START: u32 = 0xe000; const PUA_END: u32 = 0xf8ff;
    let nbsp = [0x00a0u32, 0x202fu32, 0x2007u32, 0xfeffu32];
    let mut out = String::new(); let mut prev_space=false;
    for ch in input.chars() {
        let code = ch as u32;
        if code>=PUA_START && code<=PUA_END { continue; }
        if nbsp.contains(&code) { if !prev_space { out.push(' '); prev_space=true; } continue; }
        out.push(ch); prev_space = ch==' ';
    }
    out.trim().to_string()
}

pub fn normalise_spaces(s: &str) -> String {
    let mut out=String::new(); let mut in_ws=false;
    for ch in s.chars() {
        let is_ws = ch.is_whitespace();
        if is_ws { if !in_ws { out.push(' '); in_ws=true; } } else { out.push(ch); in_ws=false; }
    }
    out
}

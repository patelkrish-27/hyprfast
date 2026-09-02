//! Port of packages/extension/prompt.ts — Stagehand prompt builders
//! Used by llm.rs to construct act/observe/extract/agent prompts
//! Same strings as JS, collapsed whitespace identical to `.replace(/\s+/g, " ")`

use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub struct ChatMessage { pub role: String, pub content: Value }

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn user_instructions(user: Option<&str>) -> String {
    match user {
        None | Some("") => String::new(),
        Some(instr) => format!("\n\n# Custom Instructions Provided by the User\nPlease keep the user's instructions in mind when performing actions. If the user's instructions are not relevant to the current task, ignore them.\nUser Instructions:\n{}", instr),
    }
}

pub fn build_extract_system_prompt(user: Option<&str>, include_screenshot: bool, print_tool: bool) -> ChatMessage {
    let base = "You are extracting content on behalf of a user. If a user asks you to extract a 'list' of information, or 'all' information, YOU MUST EXTRACT ALL OF THE INFORMATION THAT THE USER REQUESTS. You will be given: 1. An instruction 2. ";
    let detail = if include_screenshot { "A list of DOM elements to extract from and a screenshot of the current viewport to extract from. Use them together to extract content from the page." } else { "A list of DOM elements to extract from." };
    let instr = "Print the exact text from the DOM elements with all symbols, characters, and endlines as is. Print null or an empty string if no new information is found.";
    let tool = if print_tool { "ONLY print the content using the print_extracted_data tool provided. ONLY print the content using the print_extracted_data tool provided." } else { "" };
    let additional = "If a user is attempting to extract links or URLs, you MUST respond with ONLY the IDs of the link elements. Do not attempt to extract links directly from the text unless absolutely necessary. ";
    let ui = user_instructions(user);
    let raw = format!("{}{} {} {} {} {}", base, detail, instr, tool, additional, ui);
    ChatMessage { role: "system".into(), content: Value::String(collapse_ws(&raw)) }
}
pub fn build_extract_user_prompt(instruction: &str, dom: &str, print_tool: bool, screenshot: Option<Value>) -> ChatMessage {
    let mut content = if screenshot.is_some() {
        format!("Instruction: {}\nDOM: {}\nUse the screenshot together with the tree to extract.", instruction, dom)
    } else {
        format!("Instruction: {}\nDOM: {}", instruction, dom)
    };
    if print_tool { content.push_str("\nONLY print via print_extracted_data"); }
    // Stagehand supports multimodal — we return JSON array if screenshot present
    if let Some(img) = screenshot {
        ChatMessage { role: "user".into(), content: json!([{"type":"text","text":content}, img]) }
    } else {
        ChatMessage { role: "user".into(), content: json!({"type":"text","text":content}) }
    }
}
pub fn build_metadata_system_prompt() -> ChatMessage {
    ChatMessage { role: "system".into(), content: Value::String("You are an AI assistant tasked with evaluating the progress and completion status of an extraction task. Analyze the extraction response and determine if the task is completed or if more information is needed. Strictly abide by: 1. Once instruction satisfied, ALWAYS set completion true. 2. Only set false if BOTH not satisfied AND chunks remain.".into()) }
}
pub fn build_metadata_prompt(instruction: &str, response: &Value) -> ChatMessage {
    ChatMessage { role: "user".into(), content: Value::String(format!("Instruction: {}\nExtracted content: {}", instruction, serde_json::to_string_pretty(response).unwrap_or_default())) }
}

pub fn build_observe_system_prompt(user: Option<&str>, supported: Option<&[String]>, variables: Option<&Value>) -> ChatMessage {
    let actions = match supported { Some(a) if !a.is_empty() => format!("\n\nSupported actions: {}", a.join(", ")), _ => String::new() };
    let vars = variables_str(variables);
    let raw = format!("You are helping the user automate the browser by finding elements based on what the user wants to observe in the page. You will be given: 1. a instruction of elements to observe 2. a hierarchical accessibility tree showing the semantic structure of the page. The tree is a hybrid of the DOM and the accessibility tree. Return an array of elements that match the instruction if they exist, otherwise return an empty array. When returning elements, include the appropriate method from the supported actions list.{}{}. When choosing non-left click actions, provide right or middle as the argument. Each element in the accessibility tree has an ID in square brackets, like [0-18372]. The ID has two parts: frame ordinal and backend node ID. Always copy the complete ID exactly as shown inside the brackets into elementId, including the frame ordinal and hyphen. For example, if the tree shows [0-18372], return elementId \"0-18372\"; never return only \"18372\".", actions, vars);
    let content = collapse_ws(&raw);
    let ui = user_instructions(user);
    let full = if ui.is_empty() { content } else { format!("{}\n\n{}", content, ui) };
    ChatMessage { role: "system".into(), content: Value::String(full) }
}
pub fn build_observe_user_message(instruction: &str, dom: &str) -> ChatMessage {
    ChatMessage { role: "user".into(), content: Value::String(format!("instruction: {}\nAccessibility Tree: \n{}\n", instruction, dom)) }
}

pub fn build_act_system_prompt(user: Option<&str>) -> ChatMessage {
    let raw = "You are helping the user automate the browser by finding elements based on what action the user wants to take on the page You will be given: 1. a user defined instruction about what action to take 2. a hierarchical accessibility tree showing the semantic structure of the page. The tree is a hybrid of the DOM and the accessibility tree. Return the element that matches the instruction if it exists. If no element on the page matches the instruction, set `action` to null. Do not fabricate or guess an element — empty strings or placeholder values for elementId/description/method are not acceptable.";
    let content = collapse_ws(raw);
    let ui = user_instructions(user);
    let full = if ui.is_empty() { content } else { format!("{}\n\n{}", content, ui) };
    ChatMessage { role: "system".into(), content: Value::String(full) }
}

fn variables_prompt(variables: Option<&Value>) -> String {
    match variables {
        Some(Value::Object(m)) if !m.is_empty() => {
            let names = m.keys().map(|k| format!("%{}%", k)).collect::<Vec<_>>().join(", ");
            format!(" The user has provided the following variables: {} Note that these are variable names, not values. To use them, respond with the variable name inside the 'arguments' array wrapped in percentage signs (eg, %variableNameHere%) so it can be replaced before execution. ", names)
        },
        _ => String::new(),
    }
}
fn variables_str(vars: Option<&Value>) -> String {
    match vars {
        Some(Value::Object(m)) if !m.is_empty() => {
            let entries: Vec<String> = m.iter().map(|(k,v)| {
                let desc = v.get("description").and_then(|x| x.as_str()).unwrap_or("");
                if desc.is_empty() { format!("%{}%", k) } else { format!("%{}% ({})", k, desc) }
            }).collect();
            format!("\n\nAvailable variables: {}. When an action needs a dynamic value, return the matching %variableName% placeholder instead of literal", entries.join(", "))
        },
        _ => String::new(),
    }
}

pub fn build_act_prompt(action: &str, supported: &[String], variables: Option<&Value>) -> String {
    let vars = variables_prompt(variables);
    format!("Find the most relevant element to perform an action on given the following action: {}. IF AND ONLY IF the action EXPLICITLY includes the word 'dropdown' and implies choosing/selecting an option from a dropdown, ignore the 'General Instructions' section, and follow the 'Dropdown Specific Instructions' section carefully. General Instructions: Provide an action for this element such as {}. Remember buttons and links look the same. When choosing non-left click actions, provide right or middle as argument. If unrelated or no match, set `action` to null. Do not fabricate. ONLY return one action. If scrolling to position like 'halfway' or 0.75, return argument as '50%' or '75%'. If scrolling next/prev chunk, choose nextChunk/prevChunk. If key press like 'press enter', choose press method with key like 'a','Enter','Space'. Do not click on-screen keyboard. Dropdown Specific Instructions: CASE 1: element is 'select' -> choose selectOptionFromDropdown, argument exact option text, twoStep false. CASE 2: not 'select' -> click to expand: choose node most matching EVEN if StaticText not interactable, method 'click', twoStep true.{}", action, supported.join(", "), vars)
}
pub fn build_step_two_prompt(original: &str, previous: &str, supported: &[String], variables: Option<&Value>) -> String {
    let vars = variables_prompt(variables);
    format!("The original user action was: {}. You have just taken: {}. Now find the most relevant element to complete step 2 of 2. General Instructions: Provide action such as {}. Remember buttons/links similar. If unrelated/no match set null. ONLY one action. If scroll to position, use percent. If next/prev chunk, use nextChunk/prevChunk. If key press, choose press method with key like 'a','Enter','Space'.{}", original, previous, supported.join(", "), vars)
}
pub fn build_operator_system_prompt(goal: &str) -> ChatMessage {
    ChatMessage { role: "system".into(), content: Value::String(format!("You are a general-purpose agent whose job is to accomplish the user's goal across multiple model calls by running actions on the page. Your current goal: {}\nCRITICAL: MUST use provided tools (act, extract, goto, wait, navback, refresh, close). Always use tools, never just describe. Break into atomic steps, one act per turn. Only use close when complete or impossible.", goal)) }
}
pub fn build_cua_default_system_prompt() -> String { format!("You are a helpful assistant that can use a web browser. Do not ask follow up. Today's date is {}.", chrono::Utc::now().format("%Y-%m-%d")) }
pub fn build_google_cua_system_prompt() -> ChatMessage {
    ChatMessage { role: "system".into(), content: Value::String(format!("You are a general-purpose browser agent to accomplish the user's goal. Today's date is {}. You have access to search tool; however in most cases operate within provided page/url. ONLY use search if stuck. You will be given goal and steps so far. Avoid asking user for input.", chrono::Utc::now().format("%Y-%m-%d"))) }
}

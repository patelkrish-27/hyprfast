//! Stagehand protocol — Rust port of packages/protocol/schemas.ts + schema-registry.ts
//! Covers ModelName/Provider, LLM wire, Action, Act/Observe/Extract, Page/Locator, Cookies, WebMCP, metrics etc.
//! Browserbase-specific proxy types kept for fidelity but not used locally (hyprfast uses direct CDP).

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const STAGEHAND_PROTOCOL_VERSION: &str = "4.0.0";

// ---------- Model provider ----------
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ModelProvider { #[serde(rename="openai")] OpenAI, #[serde(rename="anthropic")] Anthropic, #[serde(rename="google")] Google, #[serde(rename="groq")] Groq, #[serde(rename="cerebras")] Cerebras }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    pub modelName: String, // "openai/gpt-4o-mini" etc.
    #[serde(skip_serializing_if="Option::is_none")] pub apiKey: Option<String>,
    #[serde(skip_serializing_if="Option::is_none")] pub baseURL: Option<String>,
    #[serde(skip_serializing_if="Option::is_none")] pub organization: Option<String>,
    #[serde(skip_serializing_if="Option::is_none")] pub provider: Option<ModelProvider>,
}

// re-export for llm.rs
pub type ModelName = String;

// ---------- Cookies ----------
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cookie { pub name: String, pub value: String, pub domain: String, pub path: String, pub expires: f64, pub httpOnly: bool, pub secure: bool, pub sameSite: String }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CookieParam {
    pub name: String, pub value: String,
    #[serde(skip_serializing_if="Option::is_none")] pub url: Option<String>,
    #[serde(skip_serializing_if="Option::is_none")] pub domain: Option<String>,
    #[serde(skip_serializing_if="Option::is_none")] pub path: Option<String>,
    #[serde(skip_serializing_if="Option::is_none")] pub expires: Option<f64>,
    #[serde(skip_serializing_if="Option::is_none")] pub httpOnly: Option<bool>,
    #[serde(skip_serializing_if="Option::is_none")] pub secure: Option<bool>,
    #[serde(skip_serializing_if="Option::is_none")] pub sameSite: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct CookieFilterString(pub String);
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct ClearCookieOptions {
    #[serde(skip_serializing_if="Option::is_none")] pub name: Option<Value>,
    #[serde(skip_serializing_if="Option::is_none")] pub domain: Option<Value>,
    #[serde(skip_serializing_if="Option::is_none")] pub path: Option<Value>,
}
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct DomainPolicy {
    #[serde(skip_serializing_if="Option::is_none")] pub allowedDomains: Option<Vec<String>>,
    #[serde(skip_serializing_if="Option::is_none")] pub blockedDomains: Option<Vec<String>>,
}

// ---------- LLM ----------
#[derive(Debug, Clone, Serialize, Deserialize)] #[serde(rename_all="lowercase")] pub enum LLMRole { User, Assistant }
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct LLMTextContent { #[serde(rename="type")] pub kind: String, pub text: String }
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct LLMImageContent { #[serde(rename="type")] pub kind: String, pub data: String, pub mimeType: String }
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct LLMToolUseContent { #[serde(rename="type")] pub kind: String, pub id: String, pub name: String, pub input: Value }
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct LLMToolResultContent { #[serde(rename="type")] pub kind: String, pub toolUseId: String, pub content: Vec<Value>, #[serde(skip_serializing_if="Option::is_none")] pub isError: Option<bool> }
#[derive(Debug, Clone, Serialize, Deserialize)] #[serde(untagged)] pub enum LLMMessageContentBlock { Text(LLMTextContent), Image(LLMImageContent), ToolUse(LLMToolUseContent), ToolResult(LLMToolResultContent) }
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct LLMMessage { pub role: LLMRole, pub content: Value } // union handling via Value
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct LLMTool { #[serde(rename="type")] pub kind: String, pub name: String, pub description: String, pub parameters: Value }
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct LLMClientTool { pub name: String, #[serde(skip_serializing_if="Option::is_none")] pub description: Option<String>, pub inputSchema: Value }
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct LLMUsage { pub inputTokens: u32, pub outputTokens: u32, pub totalTokens: u32, #[serde(skip_serializing_if="Option::is_none")] pub reasoningTokens: Option<u32>, #[serde(skip_serializing_if="Option::is_none")] pub cachedInputTokens: Option<u32> }
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct LLMGenerateParams { pub messages: Vec<LLMMessage>, #[serde(skip_serializing_if="Option::is_none")] pub systemPrompt: Option<String>, #[serde(skip_serializing_if="Option::is_none")] pub temperature: Option<f64>, #[serde(skip_serializing_if="Option::is_none")] pub responseFormat: Option<Value>, #[serde(skip_serializing_if="Option::is_none")] pub tools: Option<Vec<LLMClientTool>> }
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct LLMGenerateResult { pub role: LLMRole, pub content: Value, #[serde(skip_serializing_if="Option::is_none")] pub stopReason: Option<String>, #[serde(skip_serializing_if="Option::is_none")] pub usage: Option<LLMUsage>, #[serde(skip_serializing_if="Option::is_none")] pub structuredContent: Option<Value> }

// ---------- Variables ----------
#[derive(Debug, Clone, Serialize, Deserialize)] #[serde(untagged)] pub enum VariableValue { Primitive(Value), Described{ value: Value, #[serde(skip_serializing_if="Option::is_none")] description: Option<String> } }
pub type Variables = std::collections::HashMap<String, VariableValue>;

// ---------- Locator ----------
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct Locator { pub selector: String, #[serde(skip_serializing_if="Option::is_none")] pub nth: Option<u32> }
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct LocatorDescriptor { pub pageId: String, pub selector: String, #[serde(skip_serializing_if="Option::is_none")] pub nth: Option<u32> }
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct PageLocator { #[serde(skip_serializing_if="Option::is_none")] pub pageIdx: Option<u32>, #[serde(skip_serializing_if="Option::is_none")] pub url: Option<String>, #[serde(skip_serializing_if="Option::is_none")] pub title: Option<String> }

// ---------- Action ----------
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct Action {
    pub selector: String,
    pub description: String,
    #[serde(skip_serializing_if="Option::is_none")] pub method: Option<String>,
    #[serde(skip_serializing_if="Option::is_none")] pub arguments: Option<Vec<String>>,
}

// ---------- Metrics / Cache / Metadata ----------
#[derive(Debug, Clone, Serialize, Deserialize, Default)] pub struct StagehandResultUsage { #[serde(default)] pub inputTokens: u32, #[serde(default)] pub outputTokens: u32, #[serde(default)] pub reasoningTokens: u32, #[serde(default)] pub cachedInputTokens: u32, #[serde(default)] pub inferenceTimeMs: u32 }
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct CacheTokenSavings { #[serde(default)] pub inputTokens: u32, #[serde(default)] pub outputTokens: u32, #[serde(default)] pub totalTokens: u32 }
#[derive(Debug, Clone, Serialize, Deserialize)] #[serde(rename_all="SCREAMING_SNAKE_CASE")] pub enum CacheStatus { Hit, Miss, Disabled }
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct CacheMetadata { pub status: CacheStatus, #[serde(skip_serializing_if="Option::is_none")] pub count: Option<u32>, #[serde(skip_serializing_if="Option::is_none")] pub threshold: Option<u32>, #[serde(skip_serializing_if="Option::is_none")] pub missReason: Option<String>, #[serde(skip_serializing_if="Option::is_none")] pub tokensSaved: Option<CacheTokenSavings> }
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct StagehandResultMetadata { #[serde(skip_serializing_if="Option::is_none")] pub actionId: Option<String>, pub cache: CacheMetadata, pub usage: StagehandResultUsage }
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct StagehandMetrics {
    pub actPromptTokens: u32, pub actCompletionTokens: u32, pub actReasoningTokens: u32, pub actCachedInputTokens: u32, pub actInferenceTimeMs: u32,
    pub extractPromptTokens: u32, pub extractCompletionTokens: u32, pub extractReasoningTokens: u32, pub extractCachedInputTokens: u32, pub extractInferenceTimeMs: u32,
    pub observePromptTokens: u32, pub observeCompletionTokens: u32, pub observeReasoningTokens: u32, pub observeCachedInputTokens: u32, pub observeInferenceTimeMs: u32,
    pub totalPromptTokens: u32, pub totalCompletionTokens: u32, pub totalReasoningTokens: u32, pub totalCachedInputTokens: u32, pub totalInferenceTimeMs: u32,
}
impl Default for StagehandMetrics { fn default() -> Self { Self{actPromptTokens:0,actCompletionTokens:0,actReasoningTokens:0,actCachedInputTokens:0,actInferenceTimeMs:0,extractPromptTokens:0,extractCompletionTokens:0,extractReasoningTokens:0,extractCachedInputTokens:0,extractInferenceTimeMs:0,observePromptTokens:0,observeCompletionTokens:0,observeReasoningTokens:0,observeCachedInputTokens:0,observeInferenceTimeMs:0,totalPromptTokens:0,totalCompletionTokens:0,totalReasoningTokens:0,totalCachedInputTokens:0,totalInferenceTimeMs:0} } }

// ---------- Options ----------
#[derive(Debug, Clone, Serialize, Deserialize, Default)] pub struct ActOptions {
    #[serde(skip_serializing_if="Option::is_none")] pub model: Option<ModelConfig>,
    #[serde(skip_serializing_if="Option::is_none")] pub variables: Option<Variables>,
    #[serde(skip_serializing_if="Option::is_none")] pub timeout: Option<u32>,
    #[serde(skip_serializing_if="Option::is_none")] pub locator: Option<Locator>,
    #[serde(skip_serializing_if="Option::is_none")] pub ignoreLocators: Option<Vec<Locator>>,
    #[serde(skip_serializing_if="Option::is_none")] pub cache: Option<Value>,
}
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct ActResultData { pub success: bool, pub message: String, pub actionDescription: String, pub actions: Vec<Action> }
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct ActResult { pub data: ActResultData, pub metadata: StagehandResultMetadata }
#[derive(Debug, Clone, Serialize, Deserialize, Default)] pub struct ObserveOptions {
    #[serde(skip_serializing_if="Option::is_none")] pub model: Option<ModelConfig>,
    #[serde(skip_serializing_if="Option::is_none")] pub variables: Option<Variables>,
    #[serde(skip_serializing_if="Option::is_none")] pub timeout: Option<u32>,
    #[serde(skip_serializing_if="Option::is_none")] pub locator: Option<Locator>,
    #[serde(skip_serializing_if="Option::is_none")] pub ignoreLocators: Option<Vec<Locator>>,
    #[serde(skip_serializing_if="Option::is_none")] pub cache: Option<Value>,
}
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct ObserveResult { pub data: Vec<Action>, pub metadata: StagehandResultMetadata }
#[derive(Debug, Clone, Serialize, Deserialize, Default)] pub struct ExtractOptions {
    #[serde(skip_serializing_if="Option::is_none")] pub model: Option<ModelConfig>,
    #[serde(skip_serializing_if="Option::is_none")] pub timeout: Option<u32>,
    #[serde(skip_serializing_if="Option::is_none")] pub locator: Option<Locator>,
    #[serde(skip_serializing_if="Option::is_none")] pub ignoreLocators: Option<Vec<Locator>>,
    #[serde(skip_serializing_if="Option::is_none")] pub screenshot: Option<bool>,
    #[serde(skip_serializing_if="Option::is_none")] pub cache: Option<Value>,
}
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct ExtractResult { pub data: Value, pub metadata: StagehandResultMetadata }

// ---------- Page / Navigation ----------
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct PageRef { pub pageId: String, #[serde(skip_serializing_if="Option::is_none")] pub url: Option<String>, #[serde(skip_serializing_if="Option::is_none")] pub title: Option<String> }
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct NavigationResponseDescriptor { pub responseId: String, pub url: String, pub status: u32, pub statusText: String, pub headers: std::collections::HashMap<String,String>, pub fromServiceWorker: bool }
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct PageNavigationResult { pub page: PageRef, pub response: Option<NavigationResponseDescriptor> }
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct PageSnapshotOptions { #[serde(skip_serializing_if="Option::is_none")] pub includeIframes: Option<bool> }
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct SnapshotResult { pub formattedTree: String, pub xpathMap: std::collections::HashMap<String,String>, pub urlMap: std::collections::HashMap<String,String> }
#[derive(Debug, Clone, Serialize, Deserialize)] pub enum LoadState { #[serde(rename="load")] Load, #[serde(rename="domcontentloaded")] DomContentLoaded, #[serde(rename="networkidle")] NetworkIdle }
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct PageNavigationOptions { #[serde(skip_serializing_if="Option::is_none")] pub waitUntil: Option<LoadState>, #[serde(skip_serializing_if="Option::is_none")] pub timeout: Option<u32> }
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct RgbaColor { pub r: u8, pub g: u8, pub b: u8, #[serde(skip_serializing_if="Option::is_none")] pub a: Option<f64> }

// ---------- WebMCP ----------
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct WebMCPToolDescriptor { pub name: String, pub description: String, #[serde(skip_serializing_if="Option::is_none")] pub inputSchema: Option<Value>, #[serde(skip_serializing_if="Option::is_none")] pub frameId: Option<String>, #[serde(skip_serializing_if="Option::is_none")] pub backendNodeId: Option<u32> }
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct WebMCPInvocationDescriptor { pub invocationId: String, pub toolName: String, pub frameId: String, pub input: Value }
#[derive(Debug, Clone, Serialize, Deserialize)] pub enum WebMCPInvocationStatus { Completed, Canceled, Error }
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct WebMCPToolResponse { pub invocationId: String, pub status: WebMCPInvocationStatus, #[serde(skip_serializing_if="Option::is_none")] pub output: Option<Value>, #[serde(skip_serializing_if="Option::is_none")] pub errorText: Option<String> }

// ---------- RPC registry ----------
pub mod methods {
    pub const STAGEHAND_INIT: &str = "stagehand.init";
    pub const STAGEHAND_CLOSE: &str = "stagehand.close";
    pub const STAGEHAND_ACT: &str = "stagehand.act";
    pub const STAGEHAND_OBSERVE: &str = "stagehand.observe";
    pub const STAGEHAND_EXTRACT: &str = "stagehand.extract";
    pub const STAGEHAND_METRICS: &str = "stagehand.metrics";
    pub const STAGEHAND_CALLBACK_BATCH: &str = "stagehand.callback_batch";
    pub const LLM_GENERATE: &str = "llm.generate";
    // context/page/locator/response omitted for brevity — wired via CDP directly in hyprfast
    pub const ALL: &[&str] = &[STAGEHAND_INIT, STAGEHAND_CLOSE, STAGEHAND_ACT, STAGEHAND_OBSERVE, STAGEHAND_EXTRACT, STAGEHAND_METRICS, STAGEHAND_CALLBACK_BATCH, LLM_GENERATE];
}

#[derive(Debug, Clone, Serialize, Deserialize)] pub struct StagehandInitParams {
    pub protocolVersion: String,
    pub clientInfo: Value,
    #[serde(skip_serializing_if="Option::is_none")] pub browserCdpUrl: Option<String>,
    #[serde(skip_serializing_if="Option::is_none")] pub model: Option<ModelConfig>,
    #[serde(skip_serializing_if="Option::is_none")] pub systemPrompt: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct StagehandInitResult { pub ok: bool }
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct CallbackBatchParams { pub callbackSource: String, #[serde(skip_serializing_if="Option::is_none")] pub input: Option<Value>, pub options: Value }
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct CallbackBatchResult { #[serde(skip_serializing_if="Option::is_none")] pub value: Option<Value> }

// InputFile for locator.set_input_files
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct InputFilePayload { pub name: String, pub mimeType: String, pub buffer: String } // base64

pub const MAX_CALLBACK_BATCH_TIMEOUT_MS: u32 = 2_147_483_647 - 10_000;

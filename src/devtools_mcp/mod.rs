//! Chrome DevTools MCP backend — subprocess proxy.
//!
//! `chrome-devtools-mcp` owns the browser WebSocket over stdio; hyprfast
//! is a client of it. The only `connect_async` in the repo stays in
//! `browser_runtime/connection.rs` (I2). This module adds no new WebSocket
//! transport.

pub mod process;
pub mod proxy;

pub use process::{DevToolsMcpProcess, devtools_proxy_enabled, global as devtools_global};
pub use proxy::{DevToolsMcpProxy, global_proxy, proxy_block_on};

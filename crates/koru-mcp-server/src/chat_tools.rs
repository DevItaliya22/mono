// SPDX-License-Identifier: MPL-2.0

//! Chat-tool names and portable contracts shared by MCP and Koru AI.

pub use koru_ai_core::{ListChatsToolRequest, ReadChatToolRequest, SearchChatsToolRequest};

pub const CHAT_TOOL_NAMES: &[&str] = &["list_chats", "search_chats", "read_chat"];

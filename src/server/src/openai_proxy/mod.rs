//! OpenAI-compatible proxy translation helpers.
//!
//! This module owns the protocol shape conversion for providers that only
//! expose Chat Completions while a launched client expects the Responses API.
//! HTTP routing and profile lookup should live outside this folder; the code
//! here stays focused on deterministic request/response/SSE mapping.

mod chat_to_responses;
mod error;
pub mod providers;
mod reasoning_blob;
mod responses_to_chat;
mod sse;

pub use chat_to_responses::{chat_completion_to_response, ChatToResponsesStream};
pub use error::{ProxyTransformError, Result};
pub use responses_to_chat::responses_to_chat_request;
pub use sse::{encode_sse_event, ResponseStreamEvent};

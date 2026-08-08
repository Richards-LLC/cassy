//! Traced Claude API wrapper
//!
//! Provides a drop-in replacement for `claude_rs::prompt()` that automatically
//! records traces when dev mode is enabled.
//!
//! # Usage
//!
//! ```rust,ignore
//! use cas::tracing::claude_wrapper::traced_prompt;
//!
//! // Instead of:
//! // let result = claude_rs::prompt(&prompt_text, options).await?;
//!
//! // Use:
//! let result = traced_prompt(&prompt_text, options, "consolidation").await?;
//! ```
//!
//! # Integration Status
//!
//! Fully integrated - all Claude API calls use this wrapper for tracing.

use claude_rs::{QueryOptions, message::ResultMessage};

use crate::tracing::TraceTimer;
use crate::tracing::dev_tracer::DevTracer;

/// Traced version of claude_rs::prompt()
///
/// Wraps the Claude API call and records a trace if dev mode is enabled.
/// The `caller` parameter identifies the calling context (e.g., "consolidation",
/// "extraction", "hook_summary").
pub async fn traced_prompt(
    prompt: &str,
    mut options: QueryOptions,
    caller: &str,
) -> Result<ResultMessage, claude_rs::Error> {
    let timer = TraceTimer::new();

    // Extract model from options if possible (we'll use a default if not)
    let model = "claude"; // QueryOptions doesn't expose model getter easily

    options
        .env
        .get_or_insert_with(Default::default)
        .extend(crate::internal_llm::sdk_environment());
    let result = claude_rs::prompt(prompt, options).await;

    let duration_ms = timer.elapsed_ms();

    // Record trace if dev mode is enabled
    if let Some(tracer) = DevTracer::get() {
        if tracer.should_trace_claude_api() {
            match &result {
                Ok(response) => {
                    let response_text = response.text();
                    let _ = tracer.record_claude_api(
                        model,
                        prompt,
                        response_text,
                        duration_ms,
                        None, // input_tokens - not easily available from QueryResult
                        None, // output_tokens
                        None, // cost_usd
                        true,
                        None,
                        caller,
                    );
                }
                Err(e) => {
                    let _ = tracer.record_claude_api(
                        model,
                        prompt,
                        "",
                        duration_ms,
                        None,
                        None,
                        None,
                        false,
                        Some(&e.to_string()),
                        caller,
                    );
                }
            }
        }
    }

    result
}

/// Traced version with explicit model name
///
/// Use this when you know the model name and want it recorded accurately.
pub async fn traced_prompt_with_model(
    prompt: &str,
    mut options: QueryOptions,
    model: &str,
    caller: &str,
) -> Result<ResultMessage, claude_rs::Error> {
    let timer = TraceTimer::new();

    options
        .env
        .get_or_insert_with(Default::default)
        .extend(crate::internal_llm::sdk_environment());
    let result = claude_rs::prompt(prompt, options).await;

    let duration_ms = timer.elapsed_ms();

    // Record trace if dev mode is enabled
    if let Some(tracer) = DevTracer::get() {
        if tracer.should_trace_claude_api() {
            match &result {
                Ok(response) => {
                    let response_text = response.text();
                    let _ = tracer.record_claude_api(
                        model,
                        prompt,
                        response_text,
                        duration_ms,
                        None,
                        None,
                        None,
                        true,
                        None,
                        caller,
                    );
                }
                Err(e) => {
                    let _ = tracer.record_claude_api(
                        model,
                        prompt,
                        "",
                        duration_ms,
                        None,
                        None,
                        None,
                        false,
                        Some(&e.to_string()),
                        caller,
                    );
                }
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    // Tests require ai-extraction feature
}

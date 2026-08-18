//! Dev mode tracer for comprehensive operation tracing
//!
//! Provides a global singleton tracer that records all Cassy operations
//! when dev mode is enabled. Traces command execution, Claude API calls,
//! store operations, and hook events.

use std::path::Path;
use std::sync::{Mutex, OnceLock};

use chrono::Utc;

use crate::config::{Config, DevConfig};
use crate::error::MemError;
use crate::tracing::{
    BufferedObservation, ClaudeApiTrace, CommandExecutionTrace, ContextInjectionTrace,
    ExtractionTrace, HookEventTrace, RuleApplicationTrace, SkillInvocationTrace,
    StoreOperationTrace, SurfacedItem, ToolTrace, TraceBuilder, TraceEvent, TraceEventType,
    TraceStore, generate_trace_id,
};
use helpers::{generate_session_id, sanitize_args};

mod helpers;

#[cfg(test)]
mod tests;

/// Global dev tracer instance (wrapped in Option for conditional init)
static DEV_TRACER: OnceLock<Option<DevTracerInner>> = OnceLock::new();

/// Inner tracer struct that holds the actual state
struct DevTracerInner {
    /// Trace store for persistence (wrapped in Mutex for thread safety)
    store: Mutex<TraceStore>,
    /// Session ID for this CLI invocation
    session_id: String,
    /// Configuration for conditional tracing
    config: DevConfig,
}

/// Dev mode tracer for comprehensive operation tracing
pub struct DevTracer;

impl DevTracer {
    /// Get the inner tracer if initialized
    fn inner() -> Option<&'static DevTracerInner> {
        DEV_TRACER.get().and_then(|opt| opt.as_ref())
    }
}

impl DevTracer {
    /// Initialize the global dev tracer if dev mode is enabled
    ///
    /// Should be called once at CLI startup. Returns Ok(true) if tracer
    /// was initialized, Ok(false) if dev mode is disabled.
    pub fn init_global(cas_root: &Path) -> Result<bool, MemError> {
        // Only initialize once
        if DEV_TRACER.get().is_some() {
            return Ok(DEV_TRACER.get().unwrap().is_some());
        }

        // Load config to check dev mode
        let config = Config::load(cas_root).unwrap_or_default();
        let dev_config = config.dev.unwrap_or_default();

        if !dev_config.dev_mode {
            // Dev mode disabled, store None
            let _ = DEV_TRACER.set(None);
            return Ok(false);
        }

        // Create trace store
        let trace_path = cas_root.join("traces.db");
        let store = TraceStore::open(&trace_path)?;

        // Generate session ID for this CLI invocation
        let session_id = generate_session_id();

        let tracer = DevTracerInner {
            store: Mutex::new(store),
            session_id,
            config: dev_config,
        };

        let _ = DEV_TRACER.set(Some(tracer));
        Ok(true)
    }

    /// Get the global dev tracer if initialized and enabled
    pub fn get() -> Option<&'static DevTracer> {
        if DEV_TRACER.get().and_then(|opt| opt.as_ref()).is_some() {
            // Return a static reference to the unit struct
            // The actual state is accessed through Self::inner()
            Some(&DevTracer)
        } else {
            None
        }
    }

    /// Check if dev mode tracing is enabled
    pub fn is_enabled() -> bool {
        DEV_TRACER.get().map(|opt| opt.is_some()).unwrap_or(false)
    }

    /// Get the current session ID
    pub fn session_id(&self) -> &str {
        Self::inner().map(|i| i.session_id.as_str()).unwrap_or("")
    }

    /// Check if command tracing is enabled
    pub fn should_trace_commands(&self) -> bool {
        Self::inner()
            .map(|i| i.config.trace_commands)
            .unwrap_or(false)
    }

    /// Check if Claude API tracing is enabled
    pub fn should_trace_claude_api(&self) -> bool {
        Self::inner()
            .map(|i| i.config.trace_claude_api)
            .unwrap_or(false)
    }

    /// Check if hook tracing is enabled
    pub fn should_trace_hooks(&self) -> bool {
        Self::inner().map(|i| i.config.trace_hooks).unwrap_or(false)
    }

    /// Check if store operation tracing is enabled
    pub fn should_trace_store_ops(&self) -> bool {
        Self::inner()
            .map(|i| i.config.trace_store_ops)
            .unwrap_or(false)
    }

    /// Record a CLI command execution
    pub fn record_command(
        &self,
        command: &str,
        args: &[String],
        duration_ms: u64,
        success: bool,
        error: Option<&str>,
    ) -> Result<String, MemError> {
        let inner = match Self::inner() {
            Some(i) => i,
            None => return Ok(String::new()),
        };

        if !inner.config.trace_commands {
            return Ok(String::new());
        }

        let trace = CommandExecutionTrace {
            command: command.to_string(),
            args: sanitize_args(args),
            success,
            error: error.map(|s| s.to_string()),
            exit_code: if success { Some(0) } else { Some(1) },
        };

        let event = TraceEvent {
            id: generate_trace_id(),
            event_type: TraceEventType::CommandExecution,
            timestamp: Utc::now(),
            session_id: Some(inner.session_id.clone()),
            duration_ms,
            input: serde_json::to_string(&trace).unwrap_or_default(),
            output: "{}".to_string(),
            metadata: serde_json::json!({
                "command": command,
                "arg_count": args.len(),
            })
            .to_string(),
            success,
            error: error.map(|s| s.to_string()),
        };

        let id = event.id.clone();
        if let Ok(store) = inner.store.lock() {
            let _ = store.record(&event);
        }
        Ok(id)
    }

    /// Record a Claude API call
    #[allow(clippy::too_many_arguments)]
    pub fn record_claude_api(
        &self,
        model: &str,
        prompt: &str,
        response: &str,
        duration_ms: u64,
        input_tokens: Option<u32>,
        output_tokens: Option<u32>,
        cost_usd: Option<f64>,
        success: bool,
        error: Option<&str>,
        caller: &str,
    ) -> Result<String, MemError> {
        let inner = match Self::inner() {
            Some(i) => i,
            None => return Ok(String::new()),
        };

        if !inner.config.trace_claude_api {
            return Ok(String::new());
        }

        let trace = ClaudeApiTrace {
            model: model.to_string(),
            prompt: prompt.to_string(),
            response: response.to_string(),
            input_tokens,
            output_tokens,
            cost_usd,
            success,
            error: error.map(|s| s.to_string()),
            caller: caller.to_string(),
        };

        let event = TraceEvent {
            id: generate_trace_id(),
            event_type: TraceEventType::ClaudeApiCall,
            timestamp: Utc::now(),
            session_id: Some(inner.session_id.clone()),
            duration_ms,
            input: serde_json::json!({
                "model": model,
                "prompt_length": prompt.len(),
                "caller": caller,
            })
            .to_string(),
            output: serde_json::json!({
                "response_length": response.len(),
                "input_tokens": input_tokens,
                "output_tokens": output_tokens,
                "cost_usd": cost_usd,
            })
            .to_string(),
            metadata: serde_json::to_string(&trace).unwrap_or_default(),
            success,
            error: error.map(|s| s.to_string()),
        };

        let id = event.id.clone();
        if let Ok(store) = inner.store.lock() {
            let _ = store.record(&event);
        }
        Ok(id)
    }

    /// Record a store operation
    #[allow(clippy::too_many_arguments)]
    pub fn record_store_op(
        &self,
        operation: &str,
        store_type: &str,
        item_ids: &[String],
        affected: usize,
        duration_ms: u64,
        success: bool,
        error: Option<&str>,
    ) -> Result<String, MemError> {
        let inner = match Self::inner() {
            Some(i) => i,
            None => return Ok(String::new()),
        };

        if !inner.config.trace_store_ops {
            return Ok(String::new());
        }

        let trace = StoreOperationTrace {
            operation: operation.to_string(),
            store_type: store_type.to_string(),
            item_ids: item_ids.to_vec(),
            affected,
            success,
            error: error.map(|s| s.to_string()),
        };

        let event = TraceEvent {
            id: generate_trace_id(),
            event_type: TraceEventType::StoreOperation,
            timestamp: Utc::now(),
            session_id: Some(inner.session_id.clone()),
            duration_ms,
            input: serde_json::to_string(&trace).unwrap_or_default(),
            output: serde_json::json!({
                "affected": affected,
            })
            .to_string(),
            metadata: serde_json::json!({
                "operation": operation,
                "store_type": store_type,
            })
            .to_string(),
            success,
            error: error.map(|s| s.to_string()),
        };

        let id = event.id.clone();
        if let Ok(store) = inner.store.lock() {
            let _ = store.record(&event);
        }
        Ok(id)
    }

    /// Record a hook event
    #[allow(clippy::too_many_arguments)]
    pub fn record_hook(
        &self,
        hook_name: &str,
        input: &serde_json::Value,
        output: &serde_json::Value,
        context_injected: Option<&str>,
        context_tokens: Option<usize>,
        duration_ms: u64,
        success: bool,
        error: Option<&str>,
    ) -> Result<String, MemError> {
        let inner = match Self::inner() {
            Some(i) => i,
            None => return Ok(String::new()),
        };

        if !inner.config.trace_hooks {
            return Ok(String::new());
        }

        let trace = HookEventTrace {
            hook_name: hook_name.to_string(),
            input: input.clone(),
            output: output.clone(),
            context_injected: context_injected.map(|s| s.to_string()),
            context_tokens,
        };

        // Use TraceBuilder for fluent event construction
        let event = TraceBuilder::new(TraceEventType::HookEvent)
            .session(&inner.session_id)
            .input(input)
            .output(output)
            .metadata(&serde_json::to_value(&trace).unwrap_or_default())
            .duration_ms(duration_ms)
            .finish(success, error);

        let id = event.id.clone();
        if let Ok(store) = inner.store.lock() {
            let _ = store.record(&event);
        }
        Ok(id)
    }

    /// Record a rich tool trace for learning loop detection
    pub fn record_tool_trace(&self, trace: &ToolTrace) -> Result<(), MemError> {
        let inner = match Self::inner() {
            Some(i) => i,
            None => return Ok(()),
        };

        if let Ok(store) = inner.store.lock() {
            store.record_tool_trace(trace)?;
        }
        Ok(())
    }

    /// Get last tool trace for a session (for sequence tracking)
    pub fn get_last_tool_trace(&self, session_id: &str) -> Result<Option<ToolTrace>, MemError> {
        let inner = match Self::inner() {
            Some(i) => i,
            None => return Ok(None),
        };

        if let Ok(store) = inner.store.lock() {
            store.get_last_tool_trace(session_id)
        } else {
            Ok(None)
        }
    }

    /// Record a context injection event
    pub fn record_context_injection(
        &self,
        trace: &ContextInjectionTrace,
        duration_ms: u64,
    ) -> Result<String, MemError> {
        let inner = match Self::inner() {
            Some(i) => i,
            None => return Ok(String::new()),
        };

        // Use TraceBuilder with finish_success for context injection (always succeeds)
        let event = TraceBuilder::new(TraceEventType::ContextInjection)
            .session(&inner.session_id)
            .input(&serde_json::json!({
                "cwd": trace.cwd,
                "token_budget": trace.token_budget,
            }))
            .metadata(&serde_json::to_value(trace).unwrap_or_default())
            .duration_ms(duration_ms)
            .finish_success(&serde_json::json!({
                "tasks": trace.tasks_included,
                "rules": trace.rules_included,
                "skills": trace.skills_included,
                "memories": trace.memories_included,
                "pinned": trace.pinned_included,
                "total_tokens": trace.total_tokens,
                "omitted": trace.items_omitted,
            }));

        let id = event.id.clone();
        if let Ok(store) = inner.store.lock() {
            let _ = store.record(&event);
        }
        Ok(id)
    }

    /// Record a rule application event
    pub fn record_rule_application(
        &self,
        trace: &RuleApplicationTrace,
    ) -> Result<String, MemError> {
        let inner = match Self::inner() {
            Some(i) => i,
            None => return Ok(String::new()),
        };

        let event = TraceEvent {
            id: generate_trace_id(),
            event_type: TraceEventType::RuleApplication,
            timestamp: Utc::now(),
            session_id: Some(inner.session_id.clone()),
            duration_ms: 0,
            input: serde_json::json!({
                "rule_id": trace.rule_id,
                "matched_path": trace.matched_path,
            })
            .to_string(),
            output: serde_json::json!({
                "applied": trace.applied,
                "skip_reason": trace.skip_reason,
            })
            .to_string(),
            metadata: serde_json::to_string(trace).unwrap_or_default(),
            success: trace.applied,
            error: trace.skip_reason.clone(),
        };

        let id = event.id.clone();
        if let Ok(store) = inner.store.lock() {
            let _ = store.record(&event);
        }
        Ok(id)
    }

    /// Record an extraction event
    pub fn record_extraction(
        &self,
        trace: &ExtractionTrace,
        duration_ms: u64,
        success: bool,
        error: Option<&str>,
    ) -> Result<String, MemError> {
        let inner = match Self::inner() {
            Some(i) => i,
            None => return Ok(String::new()),
        };

        // Use TraceBuilder with finish_success or finish_error based on outcome
        let builder = TraceBuilder::new(TraceEventType::Extraction)
            .session(&inner.session_id)
            .input(&serde_json::json!({
                "observation_id": trace.observation_id,
                "content_length": trace.content_length,
                "method": trace.method,
            }))
            .metadata(&serde_json::to_value(trace).unwrap_or_default())
            .duration_ms(duration_ms);

        let event = if success {
            builder.finish_success(&serde_json::json!({
                "memories_extracted": trace.memories_extracted,
                "quality_score": trace.quality_score,
                "tags": trace.tags_extracted,
            }))
        } else {
            builder
                .output(&serde_json::json!({
                    "memories_extracted": trace.memories_extracted,
                    "quality_score": trace.quality_score,
                    "tags": trace.tags_extracted,
                }))
                .finish_error(error.unwrap_or("extraction failed"))
        };

        let id = event.id.clone();
        if let Ok(store) = inner.store.lock() {
            let _ = store.record(&event);
        }
        Ok(id)
    }

    /// Record a skill invocation event
    pub fn record_skill_invocation(
        &self,
        trace: &SkillInvocationTrace,
        duration_ms: u64,
        success: bool,
        error: Option<&str>,
    ) -> Result<String, MemError> {
        let inner = match Self::inner() {
            Some(i) => i,
            None => return Ok(String::new()),
        };

        let event = TraceEvent {
            id: generate_trace_id(),
            event_type: TraceEventType::SkillInvocation,
            timestamp: Utc::now(),
            session_id: Some(inner.session_id.clone()),
            duration_ms,
            input: serde_json::json!({
                "skill_id": trace.skill_id,
                "skill_name": trace.skill_name,
                "context": trace.context,
            })
            .to_string(),
            output: serde_json::json!({
                "result_summary": trace.result_summary,
            })
            .to_string(),
            metadata: serde_json::to_string(trace).unwrap_or_default(),
            success,
            error: error.map(|s| s.to_string()),
        };

        let id = event.id.clone();
        if let Ok(store) = inner.store.lock() {
            let _ = store.record(&event);
        }
        Ok(id)
    }

    /// Record a surfaced item for feedback tracking
    pub fn record_surfaced_item(
        &self,
        item_id: &str,
        item_type: &str,
        item_preview: Option<&str>,
    ) -> Result<(), MemError> {
        let inner = match Self::inner() {
            Some(i) => i,
            None => return Ok(()),
        };

        let item = SurfacedItem {
            session_id: inner.session_id.clone(),
            item_id: item_id.to_string(),
            item_type: item_type.to_string(),
            item_preview: item_preview.map(|s| s.to_string()),
            surfaced_at: Utc::now(),
            feedback_given: false,
        };

        if let Ok(store) = inner.store.lock() {
            store.record_surfaced_item(&item)?;
        }
        Ok(())
    }

    /// Get surfaced items that haven't received feedback (for nudging)
    pub fn get_unfeedback_items(&self, limit: usize) -> Result<Vec<SurfacedItem>, MemError> {
        let inner = match Self::inner() {
            Some(i) => i,
            None => return Ok(Vec::new()),
        };

        if let Ok(store) = inner.store.lock() {
            store.get_unfeedback_surfaced_items(limit)
        } else {
            Ok(Vec::new())
        }
    }

    /// Mark a surfaced item as having received feedback
    pub fn mark_feedback_given(&self, item_id: &str) -> Result<(), MemError> {
        let inner = match Self::inner() {
            Some(i) => i,
            None => return Ok(()),
        };

        if let Ok(store) = inner.store.lock() {
            store.mark_surfaced_feedback(item_id)?;
        }
        Ok(())
    }

    /// Check if an item was surfaced in the current session (for implicit feedback)
    pub fn was_surfaced_in_session(&self, item_id: &str) -> bool {
        let inner = match Self::inner() {
            Some(i) => i,
            None => return false,
        };

        if let Ok(store) = inner.store.lock() {
            store
                .was_surfaced_in_session(&inner.session_id, item_id)
                .unwrap_or(false)
        } else {
            false
        }
    }

    /// Buffer an observation for later synthesis
    pub fn buffer_observation(
        &self,
        tool_name: &str,
        file_path: Option<&str>,
        content: &str,
        exit_code: Option<i32>,
        is_error: bool,
    ) -> Result<(), MemError> {
        let inner = match Self::inner() {
            Some(i) => i,
            None => return Ok(()),
        };

        let obs = BufferedObservation {
            session_id: inner.session_id.clone(),
            tool_name: tool_name.to_string(),
            file_path: file_path.map(|s| s.to_string()),
            content: content.to_string(),
            exit_code,
            is_error,
            timestamp: Utc::now(),
        };

        if let Ok(store) = inner.store.lock() {
            store.buffer_observation(&obs)?;
        }
        Ok(())
    }

    /// Get buffered observations for the current session
    pub fn get_buffered_observations(&self) -> Result<Vec<BufferedObservation>, MemError> {
        let inner = match Self::inner() {
            Some(i) => i,
            None => return Ok(Vec::new()),
        };

        if let Ok(store) = inner.store.lock() {
            store.get_buffered_observations(&inner.session_id)
        } else {
            Ok(Vec::new())
        }
    }

    /// Get buffered observations for a specific session
    pub fn get_buffered_observations_for_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<BufferedObservation>, MemError> {
        let inner = match Self::inner() {
            Some(i) => i,
            None => return Ok(Vec::new()),
        };

        if let Ok(store) = inner.store.lock() {
            store.get_buffered_observations(session_id)
        } else {
            Ok(Vec::new())
        }
    }

    /// Clear buffered observations for the current session
    pub fn clear_observation_buffer(&self) -> Result<usize, MemError> {
        let inner = match Self::inner() {
            Some(i) => i,
            None => return Ok(0),
        };

        if let Ok(store) = inner.store.lock() {
            store.clear_observation_buffer(&inner.session_id)
        } else {
            Ok(0)
        }
    }

    /// Clear buffered observations for a specific session
    pub fn clear_observation_buffer_for_session(
        &self,
        session_id: &str,
    ) -> Result<usize, MemError> {
        let inner = match Self::inner() {
            Some(i) => i,
            None => return Ok(0),
        };

        if let Ok(store) = inner.store.lock() {
            store.clear_observation_buffer(session_id)
        } else {
            Ok(0)
        }
    }

    /// Get observation buffer count for current session
    pub fn observation_buffer_count(&self) -> Result<usize, MemError> {
        let inner = match Self::inner() {
            Some(i) => i,
            None => return Ok(0),
        };

        if let Ok(store) = inner.store.lock() {
            store.observation_buffer_count(&inner.session_id)
        } else {
            Ok(0)
        }
    }
}

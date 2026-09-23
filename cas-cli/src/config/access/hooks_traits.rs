use crate::config::*;

impl cas_core::hooks::HooksConfig for Config {
    fn token_budget(&self) -> usize {
        self.hooks.as_ref().map(|h| h.token_budget).unwrap_or(4000)
    }

    fn minimal_start(&self) -> bool {
        self.hooks
            .as_ref()
            .map(|h| h.minimal_start)
            .unwrap_or(false)
    }

    fn mcp_enabled(&self) -> bool {
        true // MCP is always enabled
    }

    fn ai_context(&self) -> bool {
        self.hooks.as_ref().map(|h| h.ai_context).unwrap_or(false)
    }

    fn ai_model(&self) -> String {
        self.hooks
            .as_ref()
            .map(|h| h.ai_model.clone())
            .unwrap_or_else(|| "claude-opus-5-5".to_string())
    }

    fn ai_fallback(&self) -> bool {
        self.hooks.as_ref().map(|h| h.ai_fallback).unwrap_or(true)
    }

    fn inject_context(&self) -> bool {
        self.hooks
            .as_ref()
            .map(|h| h.inject_context)
            .unwrap_or(true)
    }

    fn plan_mode(&self) -> cas_core::hooks::PlanModeConfig {
        let pm = self.hooks.as_ref().map(|h| &h.plan_mode);
        cas_core::hooks::PlanModeConfig {
            enabled: pm.map(|p| p.enabled).unwrap_or(true),
            token_budget: pm.map(|p| p.token_budget).unwrap_or(8000),
            task_limit: pm.map(|p| p.task_limit).unwrap_or(15),
            show_dependencies: pm.map(|p| p.show_dependencies).unwrap_or(true),
            include_closed: pm.map(|p| p.include_closed).unwrap_or(false),
        }
    }

    fn supervisor_guidance(&self) -> String {
        crate::builtins::supervisor_guidance()
    }

    fn worker_guidance(&self) -> String {
        crate::builtins::worker_guidance()
    }
}

/// Implement HooksConfig for HookConfig directly (convenience)
impl cas_core::hooks::HooksConfig for HookConfig {
    fn token_budget(&self) -> usize {
        self.token_budget
    }

    fn minimal_start(&self) -> bool {
        self.minimal_start
    }

    fn mcp_enabled(&self) -> bool {
        true // MCP is always enabled
    }

    fn ai_context(&self) -> bool {
        self.ai_context
    }

    fn ai_model(&self) -> String {
        self.ai_model.clone()
    }

    fn ai_fallback(&self) -> bool {
        self.ai_fallback
    }

    fn inject_context(&self) -> bool {
        self.inject_context
    }

    fn plan_mode(&self) -> cas_core::hooks::PlanModeConfig {
        cas_core::hooks::PlanModeConfig {
            enabled: self.plan_mode.enabled,
            token_budget: self.plan_mode.token_budget,
            task_limit: self.plan_mode.task_limit,
            show_dependencies: self.plan_mode.show_dependencies,
            include_closed: self.plan_mode.include_closed,
        }
    }

    fn supervisor_guidance(&self) -> String {
        crate::builtins::supervisor_guidance()
    }

    fn worker_guidance(&self) -> String {
        crate::builtins::worker_guidance()
    }
}

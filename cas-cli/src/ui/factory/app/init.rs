use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use cas_mux::{Mux, MuxConfig};
use ratatui::layout::Rect;

use crate::config::Config;
use crate::orchestration::names::generate_unique;
use crate::store::find_cas_root;
use crate::ui::factory::app::{
    AutoPromptConfig, EpicFocusSource, EpicState, FactoryApp, FactoryConfig, epic_branch_for_state,
    preferred_epic_focus_from_session_metadata, queue_codex_worker_intro_prompt,
    queue_supervisor_intro_prompt, resolve_epic_state_for_focus,
};
use crate::ui::factory::buffer_backend::new_hyperlink_map;
use crate::ui::factory::director::DirectorStores;
use crate::ui::factory::director::{
    DirectorData, DirectorEventDetector, PanelAreas, SidecarFocus, ViewMode,
};
use crate::ui::factory::input::{FeedbackCategory, InputMode};
use crate::ui::factory::layout::PaneGrid;
use crate::ui::factory::notification::Notifier;
use crate::ui::theme::ActiveTheme;
use crate::worktree::{WorktreeConfig, WorktreeManager};

/// Session captured by the legacy daemon before [`FactoryApp::new`] spawns
/// its initial PTYs.  The fork-first path supplies this directly, but the
/// legacy subprocess establishes it through the daemon environment.
fn factory_session_for_mux() -> Option<String> {
    std::env::var("CAS_FACTORY_SESSION").ok()
}

impl FactoryApp {
    /// Create a new factory app with the given configuration.
    pub fn new(config: FactoryConfig) -> anyhow::Result<Self> {
        use crate::worktree::GitOperations;

        let cas_dir = find_cas_root()?;

        let (supervisor_name, worker_names) = if config.minions_theme
            && config.supervisor_name.is_none()
            && config.worker_names.is_empty()
        {
            use crate::orchestration::names::{generate_minion_supervisor, generate_minion_unique};
            let sup = generate_minion_supervisor();
            let workers = generate_minion_unique(config.workers);
            (sup, workers)
        } else {
            let all_names = generate_unique(config.workers + 1);
            let sup = config
                .supervisor_name
                .unwrap_or_else(|| all_names[0].clone());
            let workers = if config.worker_names.is_empty() {
                all_names[1..].to_vec()
            } else {
                config.worker_names
            };
            (sup, workers)
        };

        let (cols, rows) = crossterm::terminal::size().unwrap_or((120, 40));

        let director_data = DirectorData::load_fast(&cas_dir)?;
        let preferred_epic_focus = preferred_epic_focus_from_session_metadata();
        let epic_state = resolve_epic_state_for_focus(&director_data, &preferred_epic_focus);

        // Detect the configured trunk once — used both for epic branch creation
        // and as the base for worker worktrees when no epic is active.
        let git_ops = GitOperations::new(config.cwd.clone());
        let trunk = Config::configured_epic_base_branch(&config.cwd)
            .unwrap_or_else(|| git_ops.detect_default_branch());

        let epic_branch = if let EpicState::Active { .. } = &epic_state {
            let branch_name = epic_branch_for_state(&director_data, &epic_state)
                .expect("active epic state must resolve a branch");
            // Warn when supervisor HEAD has drifted from trunk so operators know
            // the chosen base for the epic branch (AC: warn + surface base SHA).
            if let (Ok(head_sha), Ok(trunk_sha)) =
                (git_ops.ref_sha("HEAD"), git_ops.ref_sha(&trunk))
            {
                if head_sha != trunk_sha {
                    eprintln!(
                        "[Cassy] ⚠  supervisor HEAD ({}) differs from trunk '{}' ({}). \
                         Epic branch will be based on trunk.",
                        &head_sha[..head_sha.len().min(8)],
                        trunk,
                        &trunk_sha[..trunk_sha.len().min(8)],
                    );
                } else {
                    tracing::debug!(
                        "Epic base: trunk '{}' @ {}",
                        trunk,
                        &trunk_sha[..trunk_sha.len().min(8)]
                    );
                }
            }
            let resolved = git_ops.resolve_fresh_base(&trunk)?;
            if git_ops.create_branch_from(&branch_name, &resolved.sha)? {
                tracing::info!(
                    "Created epic branch {} from base '{}' (sha={}, behind={})",
                    branch_name,
                    resolved.branch_ref,
                    &resolved.sha[..resolved.sha.len().min(7)],
                    resolved.behind_count,
                );
            } else {
                tracing::info!("Using existing epic branch: {}", branch_name);
            }
            Some(branch_name)
        } else {
            None
        };

        let (worktree_manager, worker_cwds, cas_root_for_mux) = if config.enable_worktrees {
            let worktree_root = config
                .worktree_root
                .clone()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| {
                    let cas_config = Config::load(&cas_dir).unwrap_or_default();
                    cas_config
                        .worktrees()
                        .resolve_base_path(&config.cwd)
                        .to_string_lossy()
                        .to_string()
                });

            let wt_config = WorktreeConfig {
                enabled: true,
                base_path: worktree_root,
                branch_prefix: "factory/".to_string(),
                auto_merge: false,
                cleanup_on_close: false,
                promote_entries_on_merge: false,
            };

            // Worker worktrees base off the epic branch when one is active,
            // otherwise off the configured trunk — never off supervisor HEAD.
            let worker_base = epic_branch.as_deref().unwrap_or(trunk.as_str());

            let mut manager = WorktreeManager::new(&config.cwd, wt_config)?;
            let mut cwds = HashMap::new();
            for name in &worker_names {
                let worktree = manager.ensure_worker_worktree_from(name, worker_base)?;
                cwds.insert(name.clone(), worktree.path.clone());
            }

            (Some(manager), cwds, Some(cas_dir.clone()))
        } else {
            (None, HashMap::new(), None)
        };

        // Resolve theme: explicit config overrides auto-detection
        let cas_config = Config::load(&cas_dir).unwrap_or_default();
        let theme_config = cas_config.theme;
        let theme = ActiveTheme::resolve(theme_config.as_ref());

        // Register agent colors so TUI renders match Claude Code's Teams colors
        for (name, tc) in &config.teams_configs {
            crate::ui::theme::register_agent_color(name, &tc.agent_color);
        }

        // If a resolved supervisor spec is present (from the cascade resolver),
        // override the scalar supervisor fields so Mux::factory uses the right
        // CLI/model/effort for the supervisor pane (cas-1948).
        let (supervisor_cli, supervisor_model, supervisor_effort) =
            if let Some(ref sup_spec) = config.resolved_supervisor_spec {
                let effort_str = sup_spec
                    .effort
                    .map(|e| sup_spec.cli.backend().effort_arg(e).to_string());
                (sup_spec.cli, sup_spec.model.clone(), effort_str)
            } else {
                (
                    config.supervisor_cli,
                    config.supervisor_model.clone(),
                    config.supervisor_effort.clone(),
                )
            };

        let mux_config = MuxConfig {
            cwd: config.cwd.clone(),
            cas_root: cas_root_for_mux,
            factory_session: factory_session_for_mux(),
            worker_cwds,
            workers: worker_names.len(),
            worker_names: worker_names.clone(),
            supervisor_name: supervisor_name.clone(),
            supervisor_cli,
            worker_cli: config.worker_cli,
            supervisor_model,
            worker_model: config.worker_model.clone(),
            supervisor_effort,
            worker_effort: config.worker_effort.clone(),
            include_director: false,
            rows,
            cols,
            teams_configs: config.teams_configs,
            resolved_worker_specs: config.resolved_worker_specs,
        };

        // Cache store handles for efficient periodic refresh
        let director_stores = DirectorStores::open(&cas_dir).ok();

        let mut mux = Mux::factory(mux_config)?;
        mux.focus(&supervisor_name);

        let mut event_detector =
            DirectorEventDetector::new(worker_names.clone(), supervisor_name.clone());
        event_detector.set_stall_threshold_secs(config.stall_threshold_secs);
        // cas-728b: lets stall-candidate confirmation consult transcript
        // mtime the same way `cas factory is-wedged` does.
        event_detector.set_cas_root(cas_dir.clone());
        event_detector.initialize(&director_data);

        let notifier = Notifier::new(config.notify);
        let pane_grid = PaneGrid::new(&worker_names, &supervisor_name, config.tabbed_workers);

        let supervisor_name_for_prompt = supervisor_name.clone();
        let app = Self {
            mux,
            cas_dir,
            director_stores,
            director_data: director_data.clone(),
            unfiltered_director_data: director_data,
            director_data_loaded_at: chrono::Utc::now(),
            input_mode: InputMode::Normal,
            inject_buffer: String::new(),
            inject_target: None,
            show_help: false,
            show_changes_dialog: false,
            changes_dialog_file: None,
            changes_dialog_scroll: 0,
            changes_dialog_diff: Vec::new(),
            show_task_dialog: false,
            task_dialog_id: None,
            task_dialog_scroll: 0,
            task_dialog_max_scroll: 0,
            show_reminder_dialog: false,
            reminder_dialog_idx: None,
            reminder_dialog_scroll: 0,
            show_terminal_dialog: false,
            terminal_pane_name: None,
            show_feedback_dialog: false,
            feedback_category: FeedbackCategory::default(),
            feedback_buffer: String::new(),
            last_refresh: Instant::now(),
            refresh_interval: Duration::from_secs(2),
            last_db_fingerprint: None,
            last_git_refresh: Instant::now(),
            git_refresh_interval: Duration::from_secs(10),
            branch_visibility: Default::default(),
            theme,
            worker_names,
            supervisor_name,
            factory_session: None,
            session_created_at: None,
            supervisor_stall_after_secs: config.stall_after_secs,
            supervisor_cli: config.supervisor_cli,
            worker_cli: config.worker_cli,
            error_message: None,
            spawning_count: 0,
            select_mode: false,
            pending_workers: Vec::new(),
            error_set_at: None,
            sidecar_collapsed: false,
            worktree_manager,
            selected_worker_tab: 0,
            tabbed_workers: config.tabbed_workers,
            is_tabbed: config.tabbed_workers,
            layout_sizes: None,
            pane_grid,
            selected_pane: None,
            event_detector,
            notifier,
            current_epic_id: epic_state.epic_id().map(|s| s.to_string()),
            current_epic_source: epic_state.epic_id().map(|_| {
                preferred_epic_focus
                    .source
                    .unwrap_or(EpicFocusSource::Inference)
            }),
            epic_state,
            sidecar_focus: SidecarFocus::None,
            panels: Default::default(),
            panel_areas: PanelAreas::default(),
            view_mode: ViewMode::default(),
            detail_scroll: 0,
            agent_filter: None,
            diff_cache: Vec::new(),
            diff_scroll: 0,
            diff_metadata: None,
            syntax_highlighter: cas_diffs::highlight::SyntaxHighlighter::new(),
            diff_view_state: cas_diffs::widget::DiffViewState::default(),
            diff_display_style: cas_diffs::iter::DiffStyle::Unified,
            diff_inline_mode: cas_diffs::LineDiffType::WordAlt,
            diff_show_line_numbers: true,
            diff_expanded_hunks: std::collections::HashMap::new(),
            diff_expand_all: false,
            diff_search_mode: false,
            diff_search_query: String::new(),
            diff_search_matches: Vec::new(),
            diff_search_current: 0,
            collapsed_epics: HashSet::new(),
            collapsed_dirs: HashSet::new(),
            changes_item_types: Vec::new(),
            worker_tab_bar_area: None,
            worker_content_area: None,
            worker_areas: vec![],
            supervisor_area: None,
            sidecar_area: None,
            full_pty_content_areas: HashMap::new(),
            compact_pty_content_areas: HashMap::new(),
            terminal_cols: cols,
            terminal_rows: rows,
            auto_prompt: config.auto_prompt.clone(),
            epic_branch,
            record_enabled: config.record,
            recording_session_id: config.session_id,
            recording_start: None,
            recorded_events: Vec::new(),
            lead_session_id: config.lead_session_id,
            project_dir: config.cwd.clone(),
            session_to_pane: HashMap::new(),
            last_interrupt_time: None,
            factory_view_mode: crate::ui::factory::renderer::FactoryViewMode::default(),
            mc_focus: crate::ui::factory::renderer::MissionControlFocus::default(),
            mc_workers_area: Rect::default(),
            mc_tasks_area: Rect::default(),
            mc_changes_area: Rect::default(),
            mc_activity_area: Rect::default(),
            full_pane_hyperlinks: new_hyperlink_map(),
            compact_pane_hyperlinks: new_hyperlink_map(),
        };

        queue_supervisor_intro_prompt(
            app.cas_dir(),
            &supervisor_name_for_prompt,
            config.supervisor_cli,
            &app.worker_names,
            app.lead_session_id(),
            None,
        );
        for worker in &app.worker_names {
            queue_codex_worker_intro_prompt(app.cas_dir(), worker, config.worker_cli);
        }

        Ok(app)
    }

    /// Create a FactoryApp from daemon initialization result.
    #[allow(clippy::too_many_arguments)]
    pub fn from_init_result(
        cas_dir: PathBuf,
        mux: Mux,
        worktree_manager: Option<WorktreeManager>,
        director_data: DirectorData,
        supervisor_name: String,
        worker_names: Vec<String>,
        notify_config: crate::ui::factory::notification::NotifyConfig,
        tabbed_workers: bool,
        auto_prompt: AutoPromptConfig,
        supervisor_cli: cas_mux::SupervisorCli,
        worker_cli: cas_mux::SupervisorCli,
        cols: u16,
        rows: u16,
        record_enabled: bool,
        recording_session_id: Option<String>,
        lead_session_id: Option<String>,
        project_dir: PathBuf,
    ) -> anyhow::Result<Self> {
        let mut event_detector =
            DirectorEventDetector::new(worker_names.clone(), supervisor_name.clone());
        event_detector.initialize(&director_data);

        let preferred_epic_focus = preferred_epic_focus_from_session_metadata();
        let epic_state = resolve_epic_state_for_focus(&director_data, &preferred_epic_focus);
        let epic_branch = epic_branch_for_state(&director_data, &epic_state);

        let notifier = Notifier::new(notify_config);

        // Cache store handles for efficient periodic refresh
        let director_stores = DirectorStores::open(&cas_dir).ok();

        // Resolve theme: explicit config overrides auto-detection
        let cas_config = Config::load(&cas_dir).unwrap_or_default();
        let theme = ActiveTheme::resolve(cas_config.theme.as_ref());
        // cas-9829: activity-based stall detection threshold, sourced from
        // `.cas/config.toml` `[factory] stall_threshold_secs`.
        event_detector.set_stall_threshold_secs(cas_config.factory().stall_threshold_secs);
        // cas-728b: lets stall-candidate confirmation consult transcript
        // mtime the same way `cas factory is-wedged` does.
        event_detector.set_cas_root(cas_dir.clone());

        let app = Self {
            mux,
            cas_dir,
            director_stores,
            director_data: director_data.clone(),
            unfiltered_director_data: director_data,
            director_data_loaded_at: chrono::Utc::now(),
            input_mode: InputMode::Normal,
            inject_buffer: String::new(),
            inject_target: None,
            show_help: false,
            show_changes_dialog: false,
            changes_dialog_file: None,
            changes_dialog_scroll: 0,
            changes_dialog_diff: Vec::new(),
            show_task_dialog: false,
            task_dialog_id: None,
            task_dialog_scroll: 0,
            task_dialog_max_scroll: 0,
            show_reminder_dialog: false,
            reminder_dialog_idx: None,
            reminder_dialog_scroll: 0,
            show_terminal_dialog: false,
            terminal_pane_name: None,
            show_feedback_dialog: false,
            feedback_category: FeedbackCategory::default(),
            feedback_buffer: String::new(),
            last_refresh: Instant::now(),
            refresh_interval: Duration::from_secs(2),
            last_db_fingerprint: None,
            last_git_refresh: Instant::now(),
            git_refresh_interval: Duration::from_secs(10),
            branch_visibility: Default::default(),
            theme,
            worker_names: worker_names.clone(),
            supervisor_name: supervisor_name.clone(),
            factory_session: None,
            session_created_at: None,
            supervisor_stall_after_secs: cas_config.factory().stall_after_secs,
            supervisor_cli,
            worker_cli,
            error_message: None,
            spawning_count: 0,
            select_mode: false,
            pending_workers: Vec::new(),
            error_set_at: None,
            sidecar_collapsed: false,
            worktree_manager,
            selected_worker_tab: 0,
            tabbed_workers,
            is_tabbed: tabbed_workers,
            layout_sizes: None,
            pane_grid: PaneGrid::new(&worker_names, &supervisor_name, tabbed_workers),
            selected_pane: None,
            event_detector,
            notifier,
            current_epic_id: epic_state.epic_id().map(|s| s.to_string()),
            current_epic_source: epic_state.epic_id().map(|_| {
                preferred_epic_focus
                    .source
                    .unwrap_or(EpicFocusSource::Inference)
            }),
            epic_state,
            sidecar_focus: SidecarFocus::None,
            panels: Default::default(),
            panel_areas: PanelAreas::default(),
            view_mode: ViewMode::default(),
            detail_scroll: 0,
            agent_filter: None,
            diff_cache: Vec::new(),
            diff_scroll: 0,
            diff_metadata: None,
            syntax_highlighter: cas_diffs::highlight::SyntaxHighlighter::new(),
            diff_view_state: cas_diffs::widget::DiffViewState::default(),
            diff_display_style: cas_diffs::iter::DiffStyle::Unified,
            diff_inline_mode: cas_diffs::LineDiffType::WordAlt,
            diff_show_line_numbers: true,
            diff_expanded_hunks: std::collections::HashMap::new(),
            diff_expand_all: false,
            diff_search_mode: false,
            diff_search_query: String::new(),
            diff_search_matches: Vec::new(),
            diff_search_current: 0,
            collapsed_epics: HashSet::new(),
            collapsed_dirs: HashSet::new(),
            changes_item_types: Vec::new(),
            worker_tab_bar_area: None,
            worker_content_area: None,
            worker_areas: vec![],
            supervisor_area: None,
            sidecar_area: None,
            full_pty_content_areas: HashMap::new(),
            compact_pty_content_areas: HashMap::new(),
            terminal_cols: cols,
            terminal_rows: rows,
            auto_prompt,
            epic_branch,
            record_enabled,
            recording_session_id,
            recording_start: None,
            recorded_events: Vec::new(),
            lead_session_id,
            project_dir,
            session_to_pane: HashMap::new(),
            last_interrupt_time: None,
            factory_view_mode: crate::ui::factory::renderer::FactoryViewMode::default(),
            mc_focus: crate::ui::factory::renderer::MissionControlFocus::default(),
            mc_workers_area: Rect::default(),
            mc_tasks_area: Rect::default(),
            mc_changes_area: Rect::default(),
            mc_activity_area: Rect::default(),
            full_pane_hyperlinks: new_hyperlink_map(),
            compact_pane_hyperlinks: new_hyperlink_map(),
        };

        queue_supervisor_intro_prompt(
            app.cas_dir(),
            &supervisor_name,
            supervisor_cli,
            &worker_names,
            app.lead_session_id(),
            None,
        );
        for worker in &worker_names {
            queue_codex_worker_intro_prompt(app.cas_dir(), worker, worker_cli);
        }

        Ok(app)
    }

    /// Construct a minimal `FactoryApp` for unit testing.
    ///
    /// This does **not** spawn any PTYs, open any stores, or read any files.
    /// It is only compiled under `#[cfg(test)]` and must not be used in
    /// production paths.
    ///
    /// # Field-handling notes
    ///
    /// `FactoryApp` has ~70 fields and no `Default` impl (cas-11b0
    /// non-goal — adding `Default` to production types just for tests
    /// would leak test-shaped defaults into release builds). Every field
    /// is therefore set explicitly. The non-obvious ones, with the
    /// reasoning a future maintainer needs to add or rebalance tests:
    ///
    /// - **`mux`**: built via [`Mux::new(rows, cols)`], **not**
    ///   [`Mux::factory(MuxConfig)`]. `factory` spawns the supervisor and
    ///   worker PTYs as part of construction and is the wrong shape for
    ///   unit tests. `new` returns a PTY-less `Mux` whose pane logic
    ///   (focus, layout, render state) still works.
    /// - **`event_detector`**: constructed with
    ///   [`DirectorEventDetector::new(workers, supervisor)`] and then
    ///   `.initialize(&director_data)` is called — same sequence as the
    ///   production path. Skipping `initialize` leaves the detector
    ///   without baseline snapshots and the first `detect_changes` call
    ///   fires spurious "new task" events.
    /// - **`director_stores`**: `None`. Production code opens stores
    ///   via `DirectorStores::open(&cas_dir).ok()`. Tests intentionally
    ///   skip this so the app does not attempt to read SQLite from the
    ///   placeholder `cas_dir`; any code path that requires director
    ///   stores in a test must guard on `Some(_)` or short-circuit.
    /// - **`director_data`**: a fully-defaulted `DirectorData` literal
    ///   (every field explicit). `DirectorData` is not `Default` because
    ///   production code reads it from disk via `load_fast`; tests bake
    ///   the empty state inline.
    /// - **`notifier`**: `Notifier::new(NotifyConfig::default())`. With
    ///   the default config notifications are inert (no desktop popups,
    ///   no audio); the notifier is still constructed so any code path
    ///   that calls `.notify(...)` does not panic on `Option::None`.
    /// - **`pane_grid`**: `PaneGrid::new(&[], supervisor, tabbed=false)`.
    ///   Layout math works against an empty worker set; if a test needs
    ///   non-trivial pane layout it should construct `PaneGrid`
    ///   separately and assign.
    /// - **`theme`**: `ActiveTheme::default()` (no on-disk theme config
    ///   read). Tests that exercise theme-aware rendering should assign
    ///   a specific theme after construction.
    /// - **`worktree_manager`**: `None`. The production path may
    ///   construct a `WorktreeManager` (which touches git); tests skip
    ///   it. Any code path that worktree-walks must guard on `Some(_)`.
    /// - **`cas_dir` / `project_dir`**: `PathBuf::from("/tmp/cas-test")`.
    ///   Placeholders — they exist so paths inside the app don't panic
    ///   on `.is_some()` checks, but **must not** be assumed to exist on
    ///   disk. Tests that need a real Cassy root should use `tempfile`.
    /// - **`terminal_cols` / `terminal_rows`**: 80x24 — matches the Mux
    ///   dimensions above. Keep these in sync if you change the Mux
    ///   size; viewport math reads both.
    /// - **`epic_state` / `current_epic_id` / `epic_branch`**: Idle /
    ///   None / None. Tests that exercise EPIC-specific guard logic must
    ///   assign these explicitly after construction.
    ///
    /// If you add a field to `FactoryApp`, you must also add it here —
    /// the test constructor will fail to compile otherwise, which is the
    /// intended canary.
    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        use cas_factory::EpicState;
        use cas_mux::SupervisorCli;

        let supervisor = "test-supervisor".to_string();
        let worker_names: Vec<String> = vec![];
        let mux = Mux::new(24, 80);
        let mut event_detector =
            DirectorEventDetector::new(worker_names.clone(), supervisor.clone());
        let director_data = DirectorData {
            ready_tasks: Vec::new(),
            in_progress_tasks: Vec::new(),
            epic_tasks: Vec::new(),
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::new(),
            changes: Vec::new(),
            git_loaded: false,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        };
        event_detector.initialize(&director_data);
        let notifier = Notifier::new(cas_factory::NotifyConfig::default());
        let pane_grid = PaneGrid::new(&worker_names, &supervisor, false);

        Self {
            mux,
            cas_dir: PathBuf::from("/tmp/cas-test"),
            director_stores: None,
            director_data: director_data.clone(),
            unfiltered_director_data: director_data,
            director_data_loaded_at: chrono::Utc::now(),
            input_mode: InputMode::Normal,
            inject_buffer: String::new(),
            inject_target: None,
            show_help: false,
            show_changes_dialog: false,
            changes_dialog_file: None,
            changes_dialog_scroll: 0,
            changes_dialog_diff: Vec::new(),
            show_task_dialog: false,
            task_dialog_id: None,
            task_dialog_scroll: 0,
            task_dialog_max_scroll: 0,
            show_reminder_dialog: false,
            reminder_dialog_idx: None,
            reminder_dialog_scroll: 0,
            show_terminal_dialog: false,
            terminal_pane_name: None,
            show_feedback_dialog: false,
            feedback_category: FeedbackCategory::default(),
            feedback_buffer: String::new(),
            last_refresh: std::time::Instant::now(),
            refresh_interval: std::time::Duration::from_secs(2),
            last_db_fingerprint: None,
            last_git_refresh: std::time::Instant::now(),
            git_refresh_interval: std::time::Duration::from_secs(10),
            branch_visibility: Default::default(),
            theme: ActiveTheme::default(),
            worker_names,
            supervisor_name: supervisor,
            factory_session: None,
            session_created_at: None,
            supervisor_stall_after_secs: 600,
            supervisor_cli: SupervisorCli::Claude,
            worker_cli: SupervisorCli::Claude,
            error_message: None,
            spawning_count: 0,
            select_mode: false,
            pending_workers: Vec::new(),
            error_set_at: None,
            sidecar_collapsed: false,
            worktree_manager: None,
            selected_worker_tab: 0,
            tabbed_workers: false,
            is_tabbed: false,
            layout_sizes: None,
            pane_grid,
            selected_pane: None,
            event_detector,
            notifier,
            current_epic_id: None,
            current_epic_source: None,
            epic_state: EpicState::Idle,
            sidecar_focus: SidecarFocus::None,
            panels: Default::default(),
            panel_areas: PanelAreas::default(),
            view_mode: ViewMode::default(),
            detail_scroll: 0,
            agent_filter: None,
            diff_cache: Vec::new(),
            diff_scroll: 0,
            diff_metadata: None,
            syntax_highlighter: cas_diffs::highlight::SyntaxHighlighter::new(),
            diff_view_state: cas_diffs::widget::DiffViewState::default(),
            diff_display_style: cas_diffs::iter::DiffStyle::Unified,
            diff_inline_mode: cas_diffs::LineDiffType::WordAlt,
            diff_show_line_numbers: true,
            diff_expanded_hunks: std::collections::HashMap::new(),
            diff_expand_all: false,
            diff_search_mode: false,
            diff_search_query: String::new(),
            diff_search_matches: Vec::new(),
            diff_search_current: 0,
            collapsed_epics: std::collections::HashSet::new(),
            collapsed_dirs: std::collections::HashSet::new(),
            changes_item_types: Vec::new(),
            worker_tab_bar_area: None,
            worker_content_area: None,
            worker_areas: vec![],
            supervisor_area: None,
            sidecar_area: None,
            full_pty_content_areas: HashMap::new(),
            compact_pty_content_areas: HashMap::new(),
            terminal_cols: 80,
            terminal_rows: 24,
            auto_prompt: AutoPromptConfig::default(),
            epic_branch: None,
            record_enabled: false,
            recording_session_id: None,
            recording_start: None,
            recorded_events: Vec::new(),
            lead_session_id: None,
            project_dir: PathBuf::from("/tmp/cas-test"),
            session_to_pane: HashMap::new(),
            last_interrupt_time: None,
            factory_view_mode: crate::ui::factory::renderer::FactoryViewMode::default(),
            mc_focus: crate::ui::factory::renderer::MissionControlFocus::default(),
            mc_workers_area: Rect::default(),
            mc_tasks_area: Rect::default(),
            mc_changes_area: Rect::default(),
            mc_activity_area: Rect::default(),
            full_pane_hyperlinks: new_hyperlink_map(),
            compact_pane_hyperlinks: new_hyperlink_map(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cas_mux::SupervisorCli;

    fn env_value<'a>(config: &'a cas_mux::PtyConfig, key: &str) -> Option<&'a str> {
        config
            .env
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }

    #[test]
    fn legacy_daemon_session_reaches_factory_app_mux_config_before_spawn() {
        let _env = crate::test_support::TestEnvGuard::with_vars(&[(
            "CAS_FACTORY_SESSION",
            "legacy-codex\"session\nwith-newline",
        )]);
        let config = MuxConfig {
            cwd: PathBuf::from("/tmp/cas-legacy-session-test"),
            workers: 1,
            worker_names: vec!["legacy-codex-worker".to_string()],
            supervisor_name: "legacy-codex-supervisor".to_string(),
            factory_session: factory_session_for_mux(),
            include_director: false,
            supervisor_cli: SupervisorCli::Codex,
            worker_cli: SupervisorCli::Codex,
            ..MuxConfig::default()
        };

        for (name, pty_config) in Mux::factory_pane_configs(&config) {
            assert_eq!(
                env_value(&pty_config, "CAS_FACTORY_SESSION"),
                Some("legacy-codex\"session\nwith-newline"),
                "{name} PTY must inherit the legacy factory session"
            );
            assert!(
                pty_config.args.contains(
                    &"mcp_servers.cs.env.CAS_FACTORY_SESSION=\"legacy-codex_session_with-newline\""
                        .to_string()
                ),
                "{name} Codex cs MCP env must receive the legacy factory session; args={:?}",
                pty_config.args
            );
        }
    }
}

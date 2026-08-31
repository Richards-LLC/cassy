use crate::ui::factory::app::imports::*;

impl FactoryApp {
    // ========================================================================
    // Agent filtering
    // ========================================================================

    /// Cycle through agent filters: None → agent1 → agent2 → ... → None
    pub fn cycle_agent_filter(&mut self) {
        let agent_names: Vec<String> = self
            .director_data
            .agents
            .iter()
            .map(|a| a.name.clone())
            .collect();

        self.agent_filter = match &self.agent_filter {
            None if !agent_names.is_empty() => Some(agent_names[0].clone()),
            Some(current) => {
                let idx = agent_names.iter().position(|name| name == current);
                match idx {
                    Some(i) if i + 1 < agent_names.len() => Some(agent_names[i + 1].clone()),
                    _ => None,
                }
            }
            None => None,
        };

        crate::telemetry::track(
            "factory_agent_filter_changed",
            vec![(
                "filter_state",
                if self.agent_filter.is_some() {
                    "filtered"
                } else {
                    "all"
                },
            )],
        );
    }

    /// Get the current filter display text
    pub fn filter_display(&self) -> &str {
        self.agent_filter.as_deref().unwrap_or("All")
    }

    /// Check if a task matches the current filter
    fn task_matches_filter(&self, task: &crate::ui::factory::director::TaskSummary) -> bool {
        match &self.agent_filter {
            None => true,
            Some(filter_name) => task.assignee.as_deref() == Some(filter_name),
        }
    }

    /// Get filtered ready tasks
    pub fn filtered_ready_tasks(&self) -> Vec<&crate::ui::factory::director::TaskSummary> {
        self.director_data
            .ready_tasks
            .iter()
            .filter(|t| self.task_matches_filter(t))
            .collect()
    }

    /// Get filtered in-progress tasks
    pub fn filtered_in_progress_tasks(&self) -> Vec<&crate::ui::factory::director::TaskSummary> {
        self.director_data
            .in_progress_tasks
            .iter()
            .filter(|t| self.task_matches_filter(t))
            .collect()
    }

    // ========================================================================
    // Section collapse
    // ========================================================================

    /// Toggle a named section collapsed via PanelRegistry.
    pub fn toggle_panel_collapsed(&mut self, focus: SidecarFocus) {
        if let Some(p) = self.panels.get_mut(focus) {
            p.toggle_collapsed();
        }
    }

    /// Toggle collapse state of the selected epic (if an epic is selected)
    pub fn toggle_epic_collapse(&mut self) {
        if self.sidecar_focus != SidecarFocus::Tasks {
            return;
        }

        let Some(selected_idx) = self.panels.tasks.list_state.selected() else {
            return;
        };

        // Find what's at the selected index
        if let Some(epic_id) = self.get_epic_at_display_index(selected_idx) {
            if self.collapsed_epics.contains(&epic_id) {
                self.collapsed_epics.remove(&epic_id);
            } else {
                self.collapsed_epics.insert(epic_id);
            }
        }
    }

    /// Toggle collapse state of the selected directory (if a directory is selected)
    pub fn toggle_dir_collapse(&mut self) {
        if self.sidecar_focus != SidecarFocus::Changes {
            return;
        }

        let Some(selected_idx) = self.panels.changes.list_state.selected() else {
            return;
        };

        // Check if selected item is a directory
        if let Some(TreeItemType::Directory(dir_path)) = self.changes_item_types.get(selected_idx) {
            if self.collapsed_dirs.contains(dir_path) {
                self.collapsed_dirs.remove(dir_path);
            } else {
                self.collapsed_dirs.insert(dir_path.clone());
            }
        }
    }

    /// Toggle collapse for the current panel (epic or directory)
    pub fn toggle_collapse(&mut self) {
        match self.sidecar_focus {
            SidecarFocus::Factory => self.toggle_panel_collapsed(SidecarFocus::Factory),
            SidecarFocus::Tasks => self.toggle_epic_collapse(),
            SidecarFocus::Reminders => {
                self.panels.reminders.toggle_collapsed();
            }
            SidecarFocus::Changes => self.toggle_dir_collapse(),
            _ => {}
        }
    }

    /// Get the epic ID if the display index points to an epic header
    fn get_epic_at_display_index(&self, target_idx: usize) -> Option<String> {
        use crate::ui::factory::director::tasks::task_matches_agent_filter;

        let scoped = crate::ui::factory::director::tasks::ScopedTaskView::new(
            &self.director_data,
            self.current_epic_id.as_deref(),
        );
        let mut idx = 0;

        for group in &scoped.epic_groups {
            // Filter subtasks by agent if needed
            let visible_subtasks: Vec<_> = if let Some(agent_id) = &self.agent_filter {
                group
                    .subtasks
                    .iter()
                    .filter(|t| task_matches_agent_filter(t, Some(agent_id)))
                    .collect()
            } else {
                group.subtasks.iter().collect()
            };

            if self.agent_filter.is_some() && visible_subtasks.is_empty() {
                continue;
            }

            // Epic header row
            if idx == target_idx {
                return Some(group.epic.id.clone());
            }
            idx += 1;

            // Subtask rows (only if not collapsed)
            if !self.collapsed_epics.contains(&group.epic.id) {
                idx += visible_subtasks.len();
            }
        }

        None
    }

    // ========================================================================
    // Diff search
    // ========================================================================

    /// Check if diff search input mode is active
    pub fn is_diff_search_mode(&self) -> bool {
        self.show_changes_dialog && self.diff_search_mode
    }

    /// Start diff search mode (only in changes dialog)
    pub fn start_diff_search(&mut self) {
        if self.show_changes_dialog {
            self.diff_search_mode = true;
            self.diff_search_query.clear();
            self.diff_search_matches.clear();
            self.diff_search_current = 0;
        }
    }

    /// Handle character input in diff search mode
    pub fn handle_diff_search_char(&mut self, c: char) {
        if self.diff_search_mode {
            self.diff_search_query.push(c);
            self.update_diff_search_matches();
        }
    }

    /// Handle backspace in diff search mode
    pub fn handle_diff_search_backspace(&mut self) {
        if self.diff_search_mode {
            self.diff_search_query.pop();
            self.update_diff_search_matches();
        }
    }

    /// Cancel diff search mode without applying
    pub fn cancel_diff_search(&mut self) {
        self.diff_search_mode = false;
    }

    /// Confirm search and exit search input mode
    pub fn confirm_diff_search(&mut self) {
        if self.diff_search_mode && !self.diff_search_matches.is_empty() {
            self.diff_search_mode = false;
            self.jump_to_current_match();
        } else {
            self.diff_search_mode = false;
        }
    }

    /// Update search matches based on current query.
    ///
    /// Iterates through the diff to find matching visual line indices so
    /// `jump_to_current_match` can scroll directly to them.
    fn update_diff_search_matches(&mut self) {
        self.diff_search_matches.clear();
        self.diff_search_current = 0;

        if self.diff_search_query.is_empty() {
            return;
        }

        let Some(ref diff) = self.diff_metadata else {
            return;
        };

        let query_lower = self.diff_search_query.to_lowercase();
        let diff = diff.clone();
        let style = self.diff_display_style;

        // Iterate through all diff lines to find matches and their visual line indices
        let mut props = cas_diffs::iter::IterateOverDiffProps::new(&diff, style);
        props.expand_all = self.diff_expand_all;
        if !self.diff_expanded_hunks.is_empty() {
            props.expanded_hunks = Some(&self.diff_expanded_hunks);
        }
        let mut visual_line: usize = 0;
        let matches = &mut self.diff_search_matches;

        let check_text = |text: &str, query: &str, matches: &mut Vec<usize>, vline: usize| {
            if text.to_lowercase().contains(query) && matches.last() != Some(&vline) {
                matches.push(vline);
            }
        };

        cas_diffs::iter::iterate_over_diff(&props, |event| {
            let collapsed_before = match &event {
                cas_diffs::iter::DiffLineEvent::Context {
                    collapsed_before, ..
                }
                | cas_diffs::iter::DiffLineEvent::ContextExpanded {
                    collapsed_before, ..
                }
                | cas_diffs::iter::DiffLineEvent::Change {
                    collapsed_before, ..
                } => *collapsed_before,
            };
            if collapsed_before > 0 {
                visual_line += 1; // separator line
            }

            match &event {
                cas_diffs::iter::DiffLineEvent::Context {
                    deletion_line,
                    addition_line,
                    ..
                }
                | cas_diffs::iter::DiffLineEvent::ContextExpanded {
                    deletion_line,
                    addition_line,
                    ..
                } => {
                    if let Some(text) = diff.deletion_lines.get(deletion_line.line_index) {
                        check_text(text, &query_lower, matches, visual_line);
                    }
                    if let Some(text) = diff.addition_lines.get(addition_line.line_index) {
                        check_text(text, &query_lower, matches, visual_line);
                    }
                }
                cas_diffs::iter::DiffLineEvent::Change {
                    deletion_line,
                    addition_line,
                    ..
                } => {
                    if let Some(meta) = deletion_line {
                        if let Some(text) = diff.deletion_lines.get(meta.line_index) {
                            check_text(text, &query_lower, matches, visual_line);
                        }
                    }
                    if let Some(meta) = addition_line {
                        if let Some(text) = diff.addition_lines.get(meta.line_index) {
                            check_text(text, &query_lower, matches, visual_line);
                        }
                    }
                }
            }
            visual_line += 1;
            false
        });
    }

    /// Jump to next search match
    pub fn next_diff_match(&mut self) {
        if self.diff_search_matches.is_empty() {
            return;
        }
        self.diff_search_current = (self.diff_search_current + 1) % self.diff_search_matches.len();
        self.jump_to_current_match();
    }

    /// Jump to previous search match
    pub fn prev_diff_match(&mut self) {
        if self.diff_search_matches.is_empty() {
            return;
        }
        self.diff_search_current = if self.diff_search_current == 0 {
            self.diff_search_matches.len() - 1
        } else {
            self.diff_search_current - 1
        };
        self.jump_to_current_match();
    }

    /// Jump to the current match position
    fn jump_to_current_match(&mut self) {
        if let Some(&visual_line) = self.diff_search_matches.get(self.diff_search_current) {
            // Scroll so the match is visible (with some context above)
            let target = visual_line.saturating_sub(3);
            self.diff_view_state.scroll_offset = target;
            self.diff_scroll = target as u16;
        }
    }

    /// Get full info about the selected change (source_path, file_path, source_name, agent_name)
    fn get_selected_change_info(&self) -> Option<(PathBuf, String, String, Option<String>)> {
        let idx = self.panels.changes.list_state.selected()?;

        // Use changes_item_types to determine what's at this visual index.
        // The rendered list includes Source headers, Directory entries, and File entries,
        // so we can't use the visual index as a flat file counter.
        let item_type = self.changes_item_types.get(idx)?;
        let file_path = match item_type {
            TreeItemType::File(path) => path,
            _ => return None, // Source headers and directories can't be opened as diffs
        };

        // Walk backwards to find the nearest Source header to identify which source owns this file
        let mut source_name = None;
        for i in (0..=idx).rev() {
            if let Some(TreeItemType::Source(name)) = self.changes_item_types.get(i) {
                source_name = Some(name);
                break;
            }
        }
        let source_name = source_name?;

        // Look up the source and file in director_data
        for source in &self.director_data.changes {
            if source.source_name == *source_name {
                for change in &source.changes {
                    if change.file_path == *file_path {
                        return Some((
                            source.source_path.clone(),
                            change.file_path.clone(),
                            source.source_name.clone(),
                            source.agent_name.clone(),
                        ));
                    }
                }
            }
        }
        None
    }

    /// Open the file diff dialog for the selected file using cas-diffs DiffWidget.
    pub fn open_changes_dialog(&mut self) {
        if let Some((source_path, file_path, source_name, agent_name)) =
            self.get_selected_change_info()
        {
            // Load and parse diff using cas-diffs
            self.load_diff_metadata(&source_path, &file_path);

            // Also populate legacy diff_cache for search compatibility
            self.load_changes_dialog_diff(&source_path, &file_path);
            self.diff_cache = self.changes_dialog_diff.clone();

            // Reset navigation state
            self.diff_view_state = cas_diffs::widget::DiffViewState::default();
            self.diff_scroll = 0;
            self.diff_search_query.clear();
            self.diff_search_matches.clear();
            self.diff_search_current = 0;

            // Store file info and show as dialog overlay
            self.changes_dialog_file = Some((source_path, file_path, source_name, agent_name));
            self.show_changes_dialog = true;
        }
    }

    /// Close the file changes dialog
    pub fn close_changes_dialog(&mut self) {
        self.show_changes_dialog = false;
        self.changes_dialog_file = None;
        self.changes_dialog_diff.clear();
        self.changes_dialog_scroll = 0;
        self.diff_metadata = None;
        self.diff_view_state = cas_diffs::widget::DiffViewState::default();
        self.diff_cache.clear();
        self.diff_search_query.clear();
        self.diff_search_matches.clear();
        self.diff_search_current = 0;
        self.diff_expanded_hunks.clear();
        self.diff_expand_all = false;
    }

    /// Open or show the terminal dialog.
    ///
    /// If a shell pane already exists (hidden), shows it. Otherwise spawns a new shell.
    pub fn open_terminal_dialog(&mut self) {
        // Already visible — nothing to do
        if self.show_terminal_dialog {
            return;
        }

        // Check if a live shell pane exists (hidden)
        if let Some(ref name) = self.terminal_pane_name {
            if let Some(pane) = self.mux.get(name) {
                if !pane.has_exited() {
                    self.show_terminal_dialog = true;
                    self.input_mode = InputMode::Terminal;
                    return;
                }
            }
            // Pane gone or exited — clean up stale reference
            self.terminal_pane_name = None;
        }

        // Spawn a new shell PTY
        use cas_mux::{Pane, Pty, PtyConfig};

        let pane_name = "__terminal__";
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "bash".to_string());
        let config = PtyConfig {
            command: shell,
            args: vec![],
            cwd: Some(self.project_dir.clone()),
            env: vec![],
            env_remove: vec![],
            rows: 24,
            cols: 80,
        };

        match Pty::spawn(pane_name, config) {
            Ok(pty) => match Pane::with_pty(
                pane_name,
                PaneKind::Shell,
                pty,
                24,
                80,
                cas_mux::SupervisorCli::Claude,
            ) {
                Ok(pane) => {
                    self.mux.add_pane(pane);
                    self.terminal_pane_name = Some(pane_name.to_string());
                    self.show_terminal_dialog = true;
                    self.input_mode = InputMode::Terminal;
                }
                Err(e) => {
                    self.set_error(format!("Failed to create terminal pane: {e}"));
                }
            },
            Err(e) => {
                self.set_error(format!("Failed to spawn shell: {e}"));
            }
        }
    }

    /// Hide the terminal dialog without killing the shell process.
    pub fn hide_terminal_dialog(&mut self) {
        self.show_terminal_dialog = false;
        self.input_mode = InputMode::Normal;
    }

    /// Kill the terminal shell and close the dialog.
    pub fn kill_terminal(&mut self) {
        if let Some(name) = self.terminal_pane_name.take() {
            if let Some(mut pane) = self.mux.remove_pane(&name) {
                pane.kill();
            }
        }
        self.show_terminal_dialog = false;
        self.input_mode = InputMode::Normal;
    }

    /// Check if a background terminal shell is alive (hidden but running).
    pub fn has_background_terminal(&self) -> bool {
        if self.show_terminal_dialog {
            return false; // visible, not "background"
        }
        if let Some(ref name) = self.terminal_pane_name {
            if let Some(pane) = self.mux.get(name) {
                return !pane.has_exited();
            }
        }
        false
    }

    /// Open the feedback dialog
    pub fn open_feedback_dialog(&mut self) {
        self.show_feedback_dialog = true;
        self.feedback_category = crate::ui::factory::input::FeedbackCategory::default();
        self.feedback_buffer.clear();
        self.input_mode = InputMode::Feedback;
        crate::telemetry::track("factory_feedback_opened", vec![]);
    }

    /// Close the feedback dialog
    pub fn close_feedback_dialog(&mut self) {
        self.show_feedback_dialog = false;
        self.feedback_buffer.clear();
        self.input_mode = InputMode::Normal;
    }

    /// Save feedback to local storage (.cas/feedback/)
    pub fn save_feedback(&mut self) {
        if self.feedback_buffer.trim().is_empty() {
            return;
        }

        let category = self.feedback_category.as_str();
        let message_len = self.feedback_buffer.trim().chars().count().to_string();

        // Create feedback directory if needed
        let feedback_dir = self.cas_dir().join("feedback");
        if let Err(e) = std::fs::create_dir_all(&feedback_dir) {
            self.set_error(format!("Failed to create feedback directory: {e}"));
            crate::telemetry::track(
                "factory_feedback_submit_result",
                vec![
                    ("success", "false"),
                    ("reason", "create_dir_failed"),
                    ("category", category),
                ],
            );
            return;
        }

        // Generate timestamp-based filename
        let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
        let filename = format!("{timestamp}.json");
        let filepath = feedback_dir.join(&filename);

        // Build feedback JSON
        let feedback = serde_json::json!({
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "category": self.feedback_category.as_str(),
            "message": self.feedback_buffer.trim(),
            "supervisor": &self.supervisor_name,
            "epic_id": self.epic_state.epic_id(),
            "epic_title": self.epic_state.epic_title(),
        });

        // Write to file
        match std::fs::write(
            &filepath,
            serde_json::to_string_pretty(&feedback).unwrap_or_default(),
        ) {
            Ok(_) => {
                // Show success message briefly (will be cleared on next key press)
                self.set_error(format!("Feedback saved to {filename}"));
                crate::telemetry::track(
                    "factory_feedback_submit_result",
                    vec![
                        ("success", "true"),
                        ("category", category),
                        ("message_len", &message_len),
                    ],
                );
            }
            Err(e) => {
                self.set_error(format!("Failed to save feedback: {e}"));
                crate::telemetry::track(
                    "factory_feedback_submit_result",
                    vec![
                        ("success", "false"),
                        ("reason", "write_failed"),
                        ("category", category),
                    ],
                );
            }
        }
    }

    /// Load diff as parsed FileDiffMetadata for DiffWidget rendering.
    ///
    /// Uses two-file diff (old vs new content) to show the entire file with
    /// changes highlighted, not just the diff hunks.
    fn load_diff_metadata(&mut self, source_path: &std::path::Path, file_path: &str) {
        // Get the new (working tree) file content
        let new_content = match std::fs::read_to_string(source_path.join(file_path)) {
            Ok(c) => c,
            Err(_) => {
                self.diff_metadata = None;
                return;
            }
        };

        // Get the old (HEAD) file content via git show
        let old_content = std::process::Command::new("git")
            .args(["show", &format!("HEAD:{file_path}")])
            .current_dir(source_path)
            .output()
            .ok()
            .and_then(|out| {
                if out.status.success() {
                    Some(String::from_utf8_lossy(&out.stdout).to_string())
                } else {
                    None
                }
            })
            .unwrap_or_default();

        let metadata = cas_diffs::diff_files(file_path, &old_content, file_path, &new_content);
        self.diff_metadata = Some(metadata);
    }

    /// Load diff content for the changes dialog
    fn load_changes_dialog_diff(&mut self, source_path: &std::path::Path, file_path: &str) {
        self.changes_dialog_diff.clear();

        // Run git diff for the file
        let output = std::process::Command::new("git")
            .args(["diff", "HEAD", "--", file_path])
            .current_dir(source_path)
            .output();

        let diff_text = match output {
            Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).to_string(),
            _ => {
                // Try without HEAD (for untracked files, show the file content)
                if let Ok(content) = std::fs::read_to_string(source_path.join(file_path)) {
                    // Format as added lines
                    self.changes_dialog_diff.push(DiffLine {
                        old_line: None,
                        new_line: None,
                        content: "--- /dev/null".to_string(),
                        line_type: DiffLineType::FileHeader,
                    });
                    self.changes_dialog_diff.push(DiffLine {
                        old_line: None,
                        new_line: None,
                        content: format!("+++ b/{file_path}"),
                        line_type: DiffLineType::FileHeader,
                    });
                    for (i, line) in content.lines().enumerate() {
                        self.changes_dialog_diff.push(DiffLine {
                            old_line: None,
                            new_line: Some(i + 1),
                            content: line.to_string(),
                            line_type: DiffLineType::Added,
                        });
                    }
                    return;
                }
                return;
            }
        };

        // Parse the diff output
        self.parse_diff_to_dialog(&diff_text);
    }

    /// Parse diff text into dialog diff lines
    fn parse_diff_to_dialog(&mut self, diff_text: &str) {
        let mut old_line = 0usize;
        let mut new_line = 0usize;

        for line in diff_text.lines() {
            if line.starts_with("diff ") || line.starts_with("index ") {
                continue;
            } else if line.starts_with("---") || line.starts_with("+++") {
                self.changes_dialog_diff.push(DiffLine {
                    old_line: None,
                    new_line: None,
                    content: line.to_string(),
                    line_type: DiffLineType::FileHeader,
                });
            } else if line.starts_with("@@") {
                // Parse hunk header: @@ -old_start,old_count +new_start,new_count @@
                if let Some(caps) = line.find("@@").and_then(|start| {
                    let rest = &line[start + 2..];
                    rest.find("@@").map(|end| &rest[..end])
                }) {
                    let parts: Vec<&str> = caps.split_whitespace().collect();
                    if let Some(old_part) = parts.first() {
                        if let Some(start) = old_part.strip_prefix('-') {
                            old_line = start
                                .split(',')
                                .next()
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(1);
                        }
                    }
                    if let Some(new_part) = parts.get(1) {
                        if let Some(start) = new_part.strip_prefix('+') {
                            new_line = start
                                .split(',')
                                .next()
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(1);
                        }
                    }
                }
                self.changes_dialog_diff.push(DiffLine {
                    old_line: None,
                    new_line: None,
                    content: line.to_string(),
                    line_type: DiffLineType::HunkHeader,
                });
            } else if let Some(stripped) = line.strip_prefix('+') {
                self.changes_dialog_diff.push(DiffLine {
                    old_line: None,
                    new_line: Some(new_line),
                    content: stripped.to_string(),
                    line_type: DiffLineType::Added,
                });
                new_line += 1;
            } else if let Some(stripped) = line.strip_prefix('-') {
                self.changes_dialog_diff.push(DiffLine {
                    old_line: Some(old_line),
                    new_line: None,
                    content: stripped.to_string(),
                    line_type: DiffLineType::Removed,
                });
                old_line += 1;
            } else if line.starts_with(' ') || line.is_empty() {
                self.changes_dialog_diff.push(DiffLine {
                    old_line: Some(old_line),
                    new_line: Some(new_line),
                    content: if line.is_empty() {
                        String::new()
                    } else {
                        line[1..].to_string()
                    },
                    line_type: DiffLineType::Context,
                });
                old_line += 1;
                new_line += 1;
            }
        }
    }

    /// Scroll diff view up by one line.
    pub fn diff_scroll_up(&mut self) {
        self.diff_view_state.scroll_up(1);
        self.diff_scroll = self.diff_view_state.scroll_offset as u16;
    }

    /// Scroll diff view down by one line.
    pub fn diff_scroll_down(&mut self) {
        let max = self.diff_total_lines();
        self.diff_view_state.scroll_down(1, max);
        self.diff_scroll = self.diff_view_state.scroll_offset as u16;
    }

    /// Scroll diff view up by one page.
    pub fn diff_page_up(&mut self) {
        self.diff_view_state.page_up();
        self.diff_scroll = self.diff_view_state.scroll_offset as u16;
    }

    /// Scroll diff view down by one page.
    pub fn diff_page_down(&mut self) {
        let max = self.diff_total_lines();
        self.diff_view_state.page_down(max);
        self.diff_scroll = self.diff_view_state.scroll_offset as u16;
    }

    /// Jump to the next hunk.
    pub fn diff_next_hunk(&mut self) {
        if let Some(ref diff) = self.diff_metadata {
            let diff = diff.clone();
            self.diff_view_state
                .next_hunk(&diff, self.diff_display_style);
            self.diff_scroll = self.diff_view_state.scroll_offset as u16;
        }
    }

    /// Jump to the previous hunk.
    pub fn diff_prev_hunk(&mut self) {
        if let Some(ref diff) = self.diff_metadata {
            let diff = diff.clone();
            self.diff_view_state
                .prev_hunk(&diff, self.diff_display_style);
            self.diff_scroll = self.diff_view_state.scroll_offset as u16;
        }
    }

    /// Toggle between unified and split diff view.
    pub fn diff_toggle_style(&mut self) {
        self.diff_display_style = match self.diff_display_style {
            cas_diffs::iter::DiffStyle::Split => cas_diffs::iter::DiffStyle::Unified,
            _ => cas_diffs::iter::DiffStyle::Split,
        };
        // Reset scroll to avoid out-of-bounds after style change
        self.diff_view_state.scroll_offset = 0;
        self.diff_view_state.selected_hunk = None;
        self.diff_scroll = 0;
    }

    /// Cycle inline diff mode: WordAlt → Word → Char → None → WordAlt
    pub fn diff_cycle_inline_mode(&mut self) {
        self.diff_inline_mode = match self.diff_inline_mode {
            cas_diffs::LineDiffType::WordAlt => cas_diffs::LineDiffType::Word,
            cas_diffs::LineDiffType::Word => cas_diffs::LineDiffType::Char,
            cas_diffs::LineDiffType::Char => cas_diffs::LineDiffType::None,
            cas_diffs::LineDiffType::None => cas_diffs::LineDiffType::WordAlt,
        };
    }

    /// Toggle line number display in diff view.
    pub fn diff_toggle_line_numbers(&mut self) {
        self.diff_show_line_numbers = !self.diff_show_line_numbers;
    }

    /// Toggle expand-all collapsed context regions.
    pub fn diff_toggle_expand_all(&mut self) {
        self.diff_expand_all = !self.diff_expand_all;
        self.diff_expanded_hunks.clear();
    }

    /// Total line count for the current diff based on display style.
    fn diff_total_lines(&self) -> usize {
        self.diff_metadata
            .as_ref()
            .map(|d| match self.diff_display_style {
                cas_diffs::iter::DiffStyle::Split => d.split_line_count,
                _ => d.unified_line_count,
            })
            .unwrap_or(0)
    }

    // ========================================================================
    // Inject mode
    // ========================================================================

    /// Start inject mode for the focused pane
    pub fn start_inject_mode(&mut self) {
        if let Some(pane) = self.mux.focused() {
            let target_kind = match pane.kind() {
                PaneKind::Supervisor => "supervisor",
                PaneKind::Worker => "worker",
                PaneKind::Director => "director",
                PaneKind::Shell => "shell",
            };
            self.inject_target = Some(pane.id().to_string());
            self.inject_buffer.clear();
            self.input_mode = InputMode::Inject;
            crate::telemetry::track("factory_inject_opened", vec![("target_kind", target_kind)]);
        }
    }

    /// Cancel inject mode
    pub fn cancel_inject_mode(&mut self) {
        self.input_mode = InputMode::Normal;
        self.inject_buffer.clear();
        self.inject_target = None;
    }

    /// Execute the inject - send text to the target pane
    pub async fn execute_inject(&mut self) -> anyhow::Result<()> {
        let target = match &self.inject_target {
            Some(t) => t.clone(),
            None => {
                self.cancel_inject_mode();
                return Ok(());
            }
        };

        let text = self.inject_buffer.clone();
        if text.is_empty() {
            self.cancel_inject_mode();
            return Ok(());
        }

        let target_kind = if target == self.supervisor_name {
            "supervisor"
        } else if self.worker_names.iter().any(|name| name == &target) {
            "worker"
        } else {
            "other"
        };
        let message_len = text.chars().count().to_string();

        // Inject the prompt. Keep the buffer intact when a dirty composer
        // defers the write so the operator can clear the target and retry.
        match self.mux.inject(&target, &text).await {
            Ok(cas_mux::InjectOutcome::Delivered) => {}
            Ok(cas_mux::InjectOutcome::DeferredComposerDirty) => {
                crate::telemetry::track(
                    "factory_inject_result",
                    vec![
                        ("success", "false"),
                        ("target_kind", target_kind),
                        ("message_len", &message_len),
                    ],
                );
                return Err(anyhow::anyhow!(
                    "target composer has unsubmitted input; clear it and retry"
                ));
            }
            Err(e) => {
                crate::telemetry::track(
                    "factory_inject_result",
                    vec![
                        ("success", "false"),
                        ("target_kind", target_kind),
                        ("message_len", &message_len),
                    ],
                );
                return Err(e.into());
            }
        }

        self.cancel_inject_mode();
        crate::telemetry::track(
            "factory_inject_result",
            vec![
                ("success", "true"),
                ("target_kind", target_kind),
                ("message_len", &message_len),
            ],
        );
        Ok(())
    }

    // ========================================================================
    // Pane select mode
    // ========================================================================

    /// Enter pane select mode
    ///
    /// Initializes selection to the currently focused pane (supervisor by default).
    pub fn enter_pane_select_mode(&mut self) {
        // Initialize selection to currently focused pane, or supervisor
        let initial = self
            .mux
            .focused()
            .map(|p| p.id().to_string())
            .unwrap_or_else(|| self.supervisor_name.clone());
        self.selected_pane = Some(initial);
        self.input_mode = InputMode::PaneSelect;
        crate::telemetry::track("factory_pane_select_entered", vec![]);
    }

    /// Exit pane select mode, returning to normal
    pub fn exit_pane_select_mode(&mut self) {
        self.input_mode = InputMode::Normal;
        self.selected_pane = None;
    }

    /// Exit pane select mode, confirming the selection
    ///
    /// Focuses the selected pane and returns to normal mode.
    pub fn confirm_pane_selection(&mut self) {
        if let Some(ref pane_id) = self.selected_pane {
            // Don't focus sidecar - it's not a real pane in the mux
            if pane_id != PANE_SIDECAR {
                self.mux.focus(pane_id);
            }
        }
        self.input_mode = InputMode::Normal;
        self.selected_pane = None;
    }

    /// Exit pane select mode, canceling the selection
    pub fn cancel_pane_select_mode(&mut self) {
        self.input_mode = InputMode::Normal;
        self.selected_pane = None;
    }

    /// Enter resize mode for adjusting pane sizes
    pub fn enter_resize_mode(&mut self) {
        self.input_mode = InputMode::Resize;
        crate::telemetry::track("factory_resize_mode_entered", vec![]);
    }

    /// Exit resize mode, returning to normal
    pub fn exit_resize_mode(&mut self) {
        self.input_mode = InputMode::Normal;
    }

    /// Adjust layout sizes using a closure, then resize PTYs to match
    pub fn resize_layout<F>(&mut self, f: F)
    where
        F: FnOnce(&mut LayoutSizes),
    {
        let sizes = self.layout_sizes.get_or_insert_with(LayoutSizes::default);
        f(sizes);
        // Recalculate PTY dimensions to match new layout percentages
        let _ = self.handle_resize(self.terminal_cols, self.terminal_rows);
    }

    /// Reset layout to defaults and resize PTYs to match
    pub fn reset_layout(&mut self) {
        self.layout_sizes = None;
        let _ = self.handle_resize(self.terminal_cols, self.terminal_rows);
    }

    /// Check if resize mode is active
    pub fn is_resize_mode(&self) -> bool {
        matches!(self.input_mode, InputMode::Resize)
    }

    /// Navigate to adjacent pane in the given direction
    pub fn navigate_pane(&mut self, dir: Direction) {
        let current = match &self.selected_pane {
            Some(p) => p.as_str(),
            None => return,
        };

        if let Some(neighbor) = self.pane_grid.neighbor(current, dir) {
            self.selected_pane = Some(neighbor.to_string());
        }
    }

    /// Check if pane select mode is active
    pub fn is_pane_select_mode(&self) -> bool {
        matches!(self.input_mode, InputMode::PaneSelect)
    }

    /// Get the currently selected pane ID (in pane select mode)
    pub fn selected_pane(&self) -> Option<&str> {
        self.selected_pane.as_deref()
    }

    /// Get a reference to the pane grid
    pub fn pane_grid(&self) -> &PaneGrid {
        &self.pane_grid
    }

    // Render/operation methods are split into app_render_and_ops.rs
}

#[cfg(test)]
mod tests {
    use super::*;
    use cas_factory::TaskSummary;
    use cas_types::{Priority, TaskStatus, TaskType};
    use std::collections::HashMap;

    fn task(
        id: &str,
        task_type: TaskType,
        epic: Option<&str>,
        assignee: Option<&str>,
    ) -> TaskSummary {
        TaskSummary {
            id: id.to_string(),
            title: id.to_string(),
            status: TaskStatus::Open,
            priority: Priority::MEDIUM,
            assignee: assignee.map(str::to_string),
            task_type,
            epic: epic.map(str::to_string),
            branch: Some(format!("epic/{id}")).filter(|_| task_type == TaskType::Epic),
            updated_at: None,
            epic_verification_owner: None,
        }
    }

    #[test]
    fn get_epic_at_display_index_uses_current_epic_scoped_mapping() {
        let mut app = FactoryApp::for_test();
        app.current_epic_id = Some("cas-focused".to_string());
        app.director_data = DirectorData {
            ready_tasks: vec![
                task(
                    "cas-focused-child",
                    TaskType::Task,
                    Some("cas-focused"),
                    None,
                ),
                task(
                    "cas-foreign-child",
                    TaskType::Task,
                    Some("cas-foreign"),
                    None,
                ),
            ],
            in_progress_tasks: Vec::new(),
            epic_tasks: vec![
                task("cas-focused", TaskType::Epic, None, None),
                task("cas-foreign", TaskType::Epic, None, None),
            ],
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::new(),
            changes: Vec::new(),
            git_loaded: false,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        };

        assert_eq!(
            app.get_epic_at_display_index(0),
            Some("cas-focused".to_string())
        );
        assert_eq!(
            app.get_epic_at_display_index(1),
            None,
            "subtask row must not map to hidden foreign epic"
        );
        assert_eq!(app.get_epic_at_display_index(2), None);
    }
}

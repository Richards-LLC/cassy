use crate::mcp::tools::core::imports::*;

/// The `dep_type` both `dep_add` and `dep_remove` read (cas-96ab): one
/// parser, so the edge `dep_add` writes is the one a matching `dep_remove`
/// finds. Case and `_`/`-` are ignored; anything unrecognised means Blocks.
fn parse_dep_type(raw: &str) -> DependencyType {
    match raw.to_lowercase().replace('_', "-").as_str() {
        "related" => DependencyType::Related,
        "parent" | "parentchild" | "parent-child" => DependencyType::ParentChild,
        "discovered" | "discoveredfrom" | "discovered-from" => DependencyType::DiscoveredFrom,
        "extracted" | "extractedfrom" | "extracted-from" => DependencyType::ExtractedFrom,
        "requires-start" | "requiresstart" => DependencyType::RequiresStart,
        _ => DependencyType::Blocks,
    }
}

impl CasCore {
    pub async fn cas_task_dep_add(
        &self,
        Parameters(req): Parameters<DependencyRequest>,
    ) -> Result<CallToolResult, McpError> {
        let task_store = self.open_task_store()?;

        // Verify both tasks exist
        let mut from_task = task_store.get(&req.from_id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("From task not found: {e}")),
            data: None,
        })?;
        let to_task = task_store.get(&req.to_id).map_err(|e| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(format!("To task not found: {e}")),
            data: None,
        })?;

        let dep_type = parse_dep_type(&req.dep_type);

        let dep = Dependency {
            from_id: req.from_id.clone(),
            to_id: req.to_id.clone(),
            dep_type,
            created_at: chrono::Utc::now(),
            created_by: Some("mcp".to_string()),
        };

        // Parent-linking an existing task must establish the same durable
        // target that worktree_merge will use.  Otherwise merge lands on the
        // epic branch while close still checks the child's stale trunk target.
        //
        // cas-6fb6: use the same rule as `task update epic=`, so a task on the
        // trunk fallback or on another epic's lane follows an epic whose own
        // target is its lane (cas-940f), and a parked delivery is refused
        // rather than silently retargeted.
        let inherited_target = if dep_type == DependencyType::ParentChild
            && to_task.task_type == TaskType::Epic
        {
            let previous_parents: Vec<Task> = task_store
                .get_dependencies(&from_task.id)
                .unwrap_or_default()
                .into_iter()
                .filter(|dep| dep.dep_type == DependencyType::ParentChild && dep.to_id != to_task.id)
                .filter_map(|dep| task_store.get(&dep.to_id).ok())
                .collect();
            super::repo_context::work_target_for_task_moved_into_epic(
                &self.cas_root,
                &from_task,
                &to_task,
                &previous_parents,
                || super::update::known_epics(task_store.as_ref()),
            )
            .filter(|target| from_task.deliverables.work_target.as_ref() != Some(target))
        } else {
            None
        };
        if let Some(target) = inherited_target.as_ref() {
            super::update::refuse_retarget_of_parked_delivery(&from_task, &to_task.id, target)?;
        }
        if inherited_target.is_some() {
            super::lifecycle::proof_scope::guard_task_proof_scope(
                &self.cas_root,
                &from_task,
                super::lifecycle::proof_scope::ProofScopeOperation::ParentLink,
            )
            .map_err(|error| McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(error.to_string()),
                data: None,
            })?;
        }

        task_store.add_dependency(&dep).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to add dependency: {e}")),
            data: None,
        })?;

        if let Some(target) = inherited_target {
            from_task.deliverables.work_target = Some(target);
            from_task.updated_at = chrono::Utc::now();
            task_store.update(&from_task).map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!(
                    "Parent dependency was added, but failed to inherit the epic work target: {e}"
                )),
                data: None,
            })?;
        }

        let split_claim_cleanup = if dep_type == DependencyType::ParentChild {
            Some(crate::cloud::epic_split::release_obsolete_claims_for_task(
                &self.cas_root,
                &from_task.id,
            ))
        } else {
            None
        };

        // A late blocking edge must re-arm an open (including reopened) task.
        // Non-blocking dependency types never project lifecycle status.
        if dep_type == DependencyType::Blocks
            && from_task.status == TaskStatus::Open
            && task_store
                .get_blockers(&from_task.id)
                .map_err(|e| McpError {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from(format!(
                        "Dependency was added, but failed to evaluate blockers: {e}"
                    )),
                    data: None,
                })?
                .iter()
                .any(|blocker| blocker.id == req.to_id)
        {
            from_task.status = TaskStatus::Blocked;
            from_task.updated_at = chrono::Utc::now();
            task_store.update(&from_task).map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!(
                    "Dependency was added, but failed to mark task blocked: {e}"
                )),
                data: None,
            })?;
        }

        let mut response = format!("Added dependency: {}", describe_dependency(&dep));
        append_split_claim_cleanup(&mut response, split_claim_cleanup);
        Ok(Self::success(response))
    }

    /// Remove a dependency of a specific type between tasks (cas-6009).
    ///
    /// Uses `dep_type` to target exactly one dependency row, leaving any
    /// other-typed deps between the same pair untouched.
    pub async fn cas_task_dep_remove(
        &self,
        Parameters(req): Parameters<DependencyRequest>,
    ) -> Result<CallToolResult, McpError> {
        let task_store = self.open_task_store()?;

        let dep_type = parse_dep_type(&req.dep_type);

        let found = task_store
            .remove_dependency_of_type(&req.from_id, &req.to_id, dep_type)
            .map_err(|e| McpError {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from(format!("Failed to remove dependency: {e}")),
                data: None,
            })?;

        if !found {
            if dep_type == DependencyType::Blocks {
                let external_store = cas_store::ExternalTaskDependencyStore::open(&self.cas_root)
                    .map_err(|error| {
                    Self::error(ErrorCode::INTERNAL_ERROR, error.to_string())
                })?;
                if external_store
                    .remove(&req.from_id, &req.to_id)
                    .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?
                {
                    let mut task = task_store.get(&req.from_id).map_err(|error| {
                        Self::error(ErrorCode::INVALID_PARAMS, error.to_string())
                    })?;
                    if task.status == TaskStatus::Blocked
                        && task_store
                            .get_blockers(&req.from_id)
                            .unwrap_or_default()
                            .is_empty()
                        && external_store
                            .list_blocking_for_task(&req.from_id)
                            .unwrap_or_default()
                            .is_empty()
                    {
                        let hold = super::block_hold::read(task_store.as_ref(), &req.from_id)
                            .map_err(|error| {
                                Self::error(
                                    ErrorCode::INTERNAL_ERROR,
                                    format!(
                                        "Could not read structured blocked hold; task remains blocked: {error}"
                                    ),
                                )
                            })?;
                        if let Some(hold) = hold {
                            super::block_hold::record_auto_unblock_skipped(
                                &mut task,
                                &hold.reason,
                            );
                            task_store.update(&task).map_err(|error| {
                                Self::error(ErrorCode::INTERNAL_ERROR, error.to_string())
                            })?;
                            return Ok(Self::success(format!(
                                "Removed external dependency: {} was blocked_by {}. Task remains blocked: {}.",
                                req.from_id, req.to_id, hold.reason
                            )));
                        }
                        task.status = TaskStatus::Open;
                        task_store.update(&task).map_err(|error| {
                            Self::error(ErrorCode::INTERNAL_ERROR, error.to_string())
                        })?;
                    }
                    return Ok(Self::success(format!(
                        "Removed external dependency: {} was blocked_by {}.",
                        req.from_id, req.to_id
                    )));
                }
            }
            return Err(McpError {
                code: ErrorCode::INVALID_PARAMS,
                message: Cow::from(format!(
                    "No {:?} dependency found from {} to {}",
                    dep_type, req.from_id, req.to_id
                )),
                data: None,
            });
        }

        let split_claim_cleanup = if dep_type == DependencyType::ParentChild {
            Some(crate::cloud::epic_split::release_obsolete_claims_for_task(
                &self.cas_root,
                &req.from_id,
            ))
        } else {
            None
        };

        let removed = Dependency {
            from_id: req.from_id.clone(),
            to_id: req.to_id.clone(),
            dep_type,
            created_at: chrono::Utc::now(),
            created_by: None,
        };
        let mut response = format!("Removed dependency: {}", describe_dependency(&removed));
        append_split_claim_cleanup(&mut response, split_claim_cleanup);
        Ok(Self::success(response))
    }
}

fn append_split_claim_cleanup(response: &mut String, result: Option<Result<usize, String>>) {
    match result {
        Some(Ok(released)) if released > 0 => response.push_str(&format!(
            "\nReleased {released} obsolete split-derived Cloud claim(s)."
        )),
        Some(Err(error)) => response.push_str(&format!(
            "\n⚠️ Obsolete split Cloud claim cleanup failed: {error}. Check the Cloud claim before starting the task."
        )),
        Some(Ok(_)) | None => {}
    }
}

/// cas-ac2e: render a dependency edge in plain, unambiguous words instead of
/// a bare `from -> to` arrow.
///
/// BUG-dep-add-direction-ambiguous-output-2026-07-08.md: the edge
/// **semantics** (`from_id`/`to_id`, direction) are correct and untouched by
/// this change — `dep_add id=A to_id=B dep_type=blocks` has always created
/// "A is blocked_by B" (B must finish first). Only the OUTPUT was
/// misleading: `"Added Blocks dependency: A -> B"` reads naturally as "A
/// blocks B" (the arrow + the word "Blocks" together imply A is the
/// blocker), which is the *opposite* of the actual effect. This live-caused
/// a supervisor to file five dependency edges 180° backwards.
///
/// Every variant below states which task is which role in plain words, with
/// no arrow, so the direction can't be misread at a glance.
pub(crate) fn describe_dependency(dep: &Dependency) -> String {
    let a = &dep.from_id;
    let b = &dep.to_id;
    match dep.dep_type {
        DependencyType::Blocks => {
            format!("{a} will not start until {b} is done ({a} blocked_by {b}).")
        }
        DependencyType::RequiresStart => {
            format!("{a} requires {b} to be done before starting ({a} requires_start {b}).")
        }
        DependencyType::ParentChild => {
            format!("{a} is a child of {b} ({b} is {a}'s parent).")
        }
        DependencyType::Related => {
            format!("{a} is related to {b}.")
        }
        DependencyType::DiscoveredFrom => {
            format!("{a} was discovered while working on {b} ({a} discovered_from {b}).")
        }
        DependencyType::ExtractedFrom => {
            format!("{a} was extracted from {b} ({a} extracted_from {b}).")
        }
    }
}

#[cfg(test)]
mod describe_dependency_tests {
    use super::*;

    #[test]
    fn blocks_states_blocked_by_in_plain_words_no_arrow() {
        let dep = Dependency::new(
            "cas-a".to_string(),
            "cas-b".to_string(),
            DependencyType::Blocks,
        );
        let msg = describe_dependency(&dep);
        assert!(!msg.contains("->"), "must not contain a bare arrow: {msg}");
        assert!(
            msg.contains("blocked_by"),
            "must state blocked_by in plain words: {msg}"
        );
        assert!(
            msg.contains("cas-a will not start until cas-b is done"),
            "must spell out which task waits on which: {msg}"
        );
    }

    #[test]
    fn parent_child_names_parent_and_child_explicitly() {
        let dep = Dependency::new(
            "cas-child".to_string(),
            "cas-parent".to_string(),
            DependencyType::ParentChild,
        );
        let msg = describe_dependency(&dep);
        assert!(!msg.contains("->"), "must not contain a bare arrow: {msg}");
        assert!(msg.contains("cas-child is a child of cas-parent"));
        assert!(msg.contains("cas-parent is cas-child's parent"));
    }

    #[test]
    fn related_has_no_arrow() {
        let dep = Dependency::new(
            "cas-a".to_string(),
            "cas-b".to_string(),
            DependencyType::Related,
        );
        assert!(!describe_dependency(&dep).contains("->"));
    }

    #[test]
    fn discovered_from_and_extracted_from_have_no_arrow() {
        for dep_type in [
            DependencyType::DiscoveredFrom,
            DependencyType::ExtractedFrom,
        ] {
            let dep = Dependency::new("cas-a".to_string(), "cas-b".to_string(), dep_type);
            let msg = describe_dependency(&dep);
            assert!(
                !msg.contains("->"),
                "{dep_type:?}: must not contain arrow: {msg}"
            );
        }
    }

    #[tokio::test]
    async fn parent_link_persists_live_epic_target_without_overwriting_explicit_lane_cas_edba() {
        use cas_types::WorkTarget;
        use tempfile::TempDir;

        let root = TempDir::new().unwrap();
        let core = CasCore::with_daemon(root.path().to_path_buf(), None, None);
        let store = core.open_task_store().unwrap();
        store.init().unwrap();

        let mut epic = Task::new("cas-edba-epic".into(), "target epic".into());
        epic.task_type = TaskType::Epic;
        epic.branch = Some("epic/live-target".into());
        epic.deliverables.work_target = Some(WorkTarget {
            repo_selector: "project:cas-edba".into(),
            target_branch: "main".into(),
        });
        store.add(&epic).unwrap();

        let mut inherited = Task::new("cas-edba-inherit".into(), "trunk child".into());
        inherited.deliverables.work_target = Some(WorkTarget {
            repo_selector: "project:cas-edba".into(),
            target_branch: "main".into(),
        });
        store.add(&inherited).unwrap();

        core.cas_task_dep_add(Parameters(DependencyRequest {
            from_id: inherited.id.clone(),
            to_id: epic.id.clone(),
            dep_type: "parent".into(),
        }))
        .await
        .expect("link trunk child to epic");
        assert_eq!(
            store
                .get(&inherited.id)
                .unwrap()
                .deliverables
                .work_target
                .unwrap()
                .target_branch,
            "epic/live-target",
            "the persisted close target must equal the branch merge selected"
        );

        let mut explicit = Task::new("cas-edba-explicit".into(), "release child".into());
        explicit.deliverables.work_target = Some(WorkTarget {
            repo_selector: "project:cas-edba".into(),
            target_branch: "release/operator-selected".into(),
        });
        store.add(&explicit).unwrap();
        core.cas_task_dep_add(Parameters(DependencyRequest {
            from_id: explicit.id.clone(),
            to_id: epic.id,
            dep_type: "parent".into(),
        }))
        .await
        .expect("link explicit child to epic");
        assert_eq!(
            store
                .get(&explicit.id)
                .unwrap()
                .deliverables
                .work_target
                .unwrap()
                .target_branch,
            "release/operator-selected",
            "an explicit non-default target remains task authority"
        );
    }

    /// cas-96ab: `dep_add` and `dep_remove` read `dep_type` the same way.
    /// `dep_add` used to coerce `extracted` (and the hyphenated spellings
    /// `dep_remove` accepts) to Blocks, so the edge it wrote was not the one
    /// a matching `dep_remove` looked for.
    #[tokio::test]
    async fn dep_add_and_dep_remove_accept_the_same_dep_type_spellings_cas_96ab() {
        use tempfile::TempDir;

        let root = TempDir::new().unwrap();
        let core = CasCore::with_daemon(root.path().to_path_buf(), None, None);
        let store = core.open_task_store().unwrap();
        store.init().unwrap();
        for (spelling, expected) in [
            ("extracted", DependencyType::ExtractedFrom),
            ("extractedfrom", DependencyType::ExtractedFrom),
            ("extracted-from", DependencyType::ExtractedFrom),
            ("extracted_from", DependencyType::ExtractedFrom),
            ("parent-child", DependencyType::ParentChild),
            ("discovered-from", DependencyType::DiscoveredFrom),
            ("requires_start", DependencyType::RequiresStart),
            ("related", DependencyType::Related),
            ("blocks", DependencyType::Blocks),
        ] {
            let from = format!("cas-96ab-from-{spelling}");
            let to = format!("cas-96ab-to-{spelling}");
            store.add(&Task::new(from.clone(), "from".into())).unwrap();
            store.add(&Task::new(to.clone(), "to".into())).unwrap();
            core.cas_task_dep_add(Parameters(DependencyRequest {
                from_id: from.clone(),
                to_id: to.clone(),
                dep_type: spelling.into(),
            }))
            .await
            .unwrap_or_else(|error| panic!("dep_add {spelling}: {error:?}"));
            let stored: Vec<DependencyType> = store
                .get_dependencies(&from)
                .unwrap()
                .into_iter()
                .map(|dep| dep.dep_type)
                .collect();
            assert_eq!(stored, vec![expected], "dep_add dep_type={spelling}");
            core.cas_task_dep_remove(Parameters(DependencyRequest {
                from_id: from.clone(),
                to_id: to.clone(),
                dep_type: spelling.into(),
            }))
            .await
            .unwrap_or_else(|error| panic!("dep_remove {spelling}: {error:?}"));
            assert!(
                store.get_dependencies(&from).unwrap().is_empty(),
                "dep_remove dep_type={spelling} removes the edge dep_add wrote"
            );
        }
    }
}

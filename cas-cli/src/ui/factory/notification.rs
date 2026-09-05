//! Terminal notifications for factory mode
//!
//! Supports multiple notification backends:
//! - Native cross-platform notifications via notify-rust (macOS, Linux, Windows)
//! - Terminal bell (\x07)
//! - iTerm2 inline notifications

use std::io::{self, Write};

use notify_rust::{Notification, Timeout, Urgency};

use crate::ui::factory::director::DirectorEvent;

// Re-export from cas-factory for backward compatibility
pub use cas_factory::{NotifyBackend, NotifyConfig};

/// Notification metadata for different event types
struct NotificationMeta {
    title: &'static str,
    icon: &'static str,
    urgency: Urgency,
}

/// Notification manager
pub struct Notifier {
    config: NotifyConfig,
}

impl Notifier {
    /// Create a new notifier with the given config
    pub fn new(config: NotifyConfig) -> Self {
        Self { config }
    }

    /// Check if notifications are enabled
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Get notification metadata for an event type
    fn event_meta(event: &DirectorEvent) -> Option<NotificationMeta> {
        match event {
            DirectorEvent::SupervisorStalled { .. } => Some(NotificationMeta {
                title: "Supervisor Stalled",
                icon: "dialog-warning",
                urgency: Urgency::Normal,
            }),
            DirectorEvent::TaskCompleted { .. } => Some(NotificationMeta {
                title: "Task Completed",
                icon: "emblem-ok-symbolic",
                urgency: Urgency::Normal,
            }),
            DirectorEvent::TaskBlocked { .. } => Some(NotificationMeta {
                title: "Task Blocked",
                icon: "dialog-warning",
                urgency: Urgency::Normal,
            }),
            DirectorEvent::WorkerIdle { .. } => Some(NotificationMeta {
                title: "Worker Idle",
                icon: "dialog-information",
                urgency: Urgency::Low,
            }),
            // Only surface a desktop notification once the stall is
            // escalated to the supervisor — the first-detection auto-nudge
            // is worker-directed and low-stakes, so it stays out of the
            // desktop channel to avoid alert fatigue (cas-9829).
            DirectorEvent::WorkerStalled { escalate, .. } if *escalate => Some(NotificationMeta {
                title: "Worker Stalled",
                icon: "dialog-warning",
                urgency: Urgency::Normal,
            }),
            DirectorEvent::WorkerStalled { .. } => None,
            DirectorEvent::EpicCompleted { .. } => Some(NotificationMeta {
                title: "Epic Completed!",
                icon: "starred",
                urgency: Urgency::Normal,
            }),
            DirectorEvent::EpicStarted { .. } => Some(NotificationMeta {
                title: "Epic Started",
                icon: "emblem-synchronizing",
                urgency: Urgency::Low,
            }),
            DirectorEvent::EpicAllSubtasksClosed { .. } => Some(NotificationMeta {
                title: "Epic Ready to Close",
                icon: "starred",
                urgency: Urgency::Normal,
            }),
            // Don't notify for these events
            DirectorEvent::TaskAssigned { .. } | DirectorEvent::AgentRegistered { .. } => None,
        }
    }

    /// Send a notification for a director event
    pub fn notify_event(&self, event: &DirectorEvent) {
        if !self.config.enabled {
            return;
        }

        let Some(meta) = Self::event_meta(event) else {
            return;
        };

        let body = match event {
            DirectorEvent::TaskCompleted {
                task_id,
                task_title,
                worker,
            } => {
                format!("{worker} completed {task_id} ({task_title})")
            }
            DirectorEvent::TaskBlocked {
                task_id,
                task_title,
                worker,
            } => {
                format!("{worker} blocked on {task_id} ({task_title})")
            }
            DirectorEvent::WorkerIdle {
                worker,
                active_task: Some(task),
            } => {
                if let Some(reason) = task.close_rejected_reason.as_deref() {
                    format!(
                        "{worker} idle; {} is {} (close rejected: {reason})",
                        task.task_id, task.task_status
                    )
                } else {
                    format!("{worker} idle; {} is {}", task.task_id, task.task_status)
                }
            }
            DirectorEvent::WorkerIdle { worker, .. } => {
                format!("{worker} is waiting for work")
            }
            DirectorEvent::WorkerStalled {
                worker,
                elapsed_secs,
                escalate: true,
                ..
            } => {
                format!(
                    "{worker} requires attention after {}m without activity",
                    elapsed_secs / 60
                )
            }
            DirectorEvent::EpicCompleted { epic_id } => {
                format!("All tasks in {epic_id} are done")
            }
            DirectorEvent::EpicStarted {
                epic_id,
                epic_title,
            } => {
                format!("Started {epic_id} - {epic_title}")
            }
            _ => return,
        };

        self.send(meta.title, &body, meta.urgency, meta.icon);
    }

    /// Send a custom notification
    pub fn notify(&self, title: &str, body: &str) {
        if !self.config.enabled {
            return;
        }
        self.send(title, body, Urgency::Normal, "dialog-information");
    }

    /// Send a notification about a worker crash
    pub fn notify_crash(&self, worker: &str, exit_info: &str) {
        if !self.config.enabled {
            return;
        }
        self.send(
            "Worker Crashed",
            &format!("{worker} {exit_info}"),
            Urgency::Critical,
            "dialog-error",
        );
    }

    /// Send the notification using the configured backend
    fn send(&self, title: &str, body: &str, urgency: Urgency, icon: &str) {
        let result = match self.config.backend {
            NotifyBackend::Native => self.send_native(title, body, urgency, icon),
            NotifyBackend::Bell => {
                self.send_bell();
                Ok(())
            }
            NotifyBackend::ITerm2 => {
                self.send_iterm2(title, body);
                Ok(())
            }
        };

        // If native notification failed, fall back to bell
        if let Err(e) = result {
            tracing::debug!("Native notification failed, falling back to bell: {}", e);
            self.send_bell();
        }

        // Also send bell if configured (and not already the primary backend)
        if self.config.also_bell && self.config.backend != NotifyBackend::Bell {
            self.send_bell();
        }
    }

    /// Send notification via notify-rust (cross-platform)
    fn send_native(
        &self,
        title: &str,
        body: &str,
        urgency: Urgency,
        icon: &str,
    ) -> Result<(), String> {
        let mut notification = Notification::new();
        notification
            .summary(title)
            .body(body)
            .appname("Cassy Factory")
            .timeout(Timeout::Milliseconds(5000));

        // Set icon and urgency (only effective on Linux)
        #[cfg(target_os = "linux")]
        {
            notification.icon(icon);
            notification.urgency(urgency);
        }

        // Suppress unused variable warnings on non-Linux
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (urgency, icon);
        }

        notification
            .show()
            .map(|_| ())
            .map_err(|e| format!("notify-rust error: {e}"))
    }

    /// Send terminal bell
    fn send_bell(&self) {
        let _ = io::stdout().write_all(b"\x07");
        let _ = io::stdout().flush();
    }

    /// Send iTerm2 notification
    fn send_iterm2(&self, title: &str, body: &str) {
        // iTerm2 notification escape sequence: \e]9;message\a
        let message = format!("{title}: {body}");
        let escape = format!("\x1b]9;{message}\x07");
        let _ = io::stdout().write_all(escape.as_bytes());
        let _ = io::stdout().flush();
    }
}

#[cfg(test)]
mod tests {
    use crate::ui::factory::notification::*;

    #[test]
    fn test_notify_backend_detection() {
        let backend = NotifyBackend::detect();
        // Should detect a valid backend
        assert!(matches!(
            backend,
            NotifyBackend::Native | NotifyBackend::ITerm2 | NotifyBackend::Bell
        ));
    }

    #[test]
    fn test_notifier_disabled_by_default() {
        let notifier = Notifier::new(NotifyConfig::default());
        assert!(!notifier.is_enabled());
    }

    #[test]
    fn test_event_notification_metadata() {
        let completed = DirectorEvent::TaskCompleted {
            task_id: "cas-123".to_string(),
            task_title: "Test".to_string(),
            worker: "swift-fox".to_string(),
        };
        let meta = Notifier::event_meta(&completed).unwrap();
        assert_eq!(meta.title, "Task Completed");
        assert!(matches!(meta.urgency, Urgency::Normal));

        // TaskAssigned should return None (no notification)
        let assigned = DirectorEvent::TaskAssigned {
            task_id: "cas-123".to_string(),
            task_title: "Test".to_string(),
            worker: "swift-fox".to_string(),
        };
        assert!(Notifier::event_meta(&assigned).is_none());
    }

    #[test]
    fn test_event_notification_content() {
        let event = DirectorEvent::TaskCompleted {
            task_id: "cas-123".to_string(),
            task_title: "Test Task".to_string(),
            worker: "swift-fox".to_string(),
        };
        assert!(event.description().contains("swift-fox"));
        assert!(event.description().contains("cas-123"));
    }

    /// cas-9829: the first-detection auto-nudge (escalate=false) is
    /// worker-directed and low-stakes — it must NOT produce a desktop
    /// notification (would be alert fatigue for something the director is
    /// already handling automatically). The escalation (escalate=true) DOES
    /// need a desktop notification since the supervisor must act.
    #[test]
    fn test_9829_worker_stalled_notification_only_on_escalation() {
        let nudge = DirectorEvent::WorkerStalled {
            worker: "swift-fox".to_string(),
            task_id: "cas-0b7d".to_string(),
            elapsed_secs: 310,
            escalate: false,
        };
        assert!(
            Notifier::event_meta(&nudge).is_none(),
            "the one-shot nudge must not trigger a desktop notification"
        );

        let escalation = DirectorEvent::WorkerStalled {
            worker: "swift-fox".to_string(),
            task_id: "cas-0b7d".to_string(),
            elapsed_secs: 620,
            escalate: true,
        };
        let meta = Notifier::event_meta(&escalation)
            .expect("escalation must trigger a desktop notification");
        assert_eq!(meta.title, "Worker Stalled");
    }
}

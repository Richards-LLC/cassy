//! Distributed Factory E2E Tests
//!
//! End-to-end tests proving distributed factory works with cloud sync.
//! Tests factory mode with multiple agents coordinating across "machines"
//! (simulated via separate CAS directories).
//!
//! # Test Scenarios
//! 1. Two separate CAS directories on same machine (simulating two machines)
//! 2. Both "machines" sync to the same cloud account (when credentials available)
//! 3. Factory registration, agent sync, task claiming, prompt delivery
//!
//! # Running
//! ```bash
//! # Run all distributed factory tests
//! cargo test --test integration_factory distributed_factory_test::
//!
//! # Run against the real cloud (cas-c7c7): a 0600 cloud.json or token file
//! CAS_TEST_CLOUD_TOKEN_FILE=<path> CAS_TEST_CLOUD_TRANSCRIPT_DIR=<dir> cargo nextest run \
//!     -p cas --test integration_factory -E 'test(two_machine) | test(test_cloud_)'
//! ```

use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use tempfile::TempDir;

use cas::store::{AgentStore, TaskStore, init_cas_dir, open_agent_store, open_task_store};
use cas::types::{Agent, AgentStatus, AgentType, Priority, Task, TaskStatus};

// Required for cloud tests
#[allow(unused_imports)]
use cas::cloud;

/// Test environment representing a simulated "machine"
struct TestMachine {
    #[allow(dead_code)]
    temp_dir: TempDir,
    cas_dir: PathBuf,
    name: String,
}

impl TestMachine {
    fn new(name: &str) -> Self {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let cas_dir = init_cas_dir(temp_dir.path()).expect("Failed to init CAS dir");
        Self {
            temp_dir,
            cas_dir,
            name: name.to_string(),
        }
    }

    fn agent_store(&self) -> Arc<dyn AgentStore> {
        open_agent_store(&self.cas_dir).expect("Failed to open agent store")
    }

    fn task_store(&self) -> Arc<dyn TaskStore> {
        open_task_store(&self.cas_dir).expect("Failed to open task store")
    }

    fn register_agent(&self, agent_name: &str, agent_type: AgentType) -> Agent {
        let store = self.agent_store();
        let id = Agent::generate_fallback_id();
        let mut agent = Agent::new(id.clone(), agent_name.to_string());
        agent.agent_type = agent_type;
        agent.machine_id = Some(self.name.clone());
        store.register(&agent).expect("Failed to register agent");
        agent
    }

    fn create_task(&self, title: &str) -> Task {
        let store = self.task_store();
        let id = store.generate_id().expect("Failed to generate task ID");
        let task = Task::new(id, title.to_string());
        store.add(&task).expect("Failed to add task");
        task
    }
}

// =============================================================================
// Local Multi-Agent Tests (No Cloud Required)
// =============================================================================

#[path = "distributed_factory_test_cases/tests.rs"]
mod tests;

#[path = "distributed_factory_test_cases/peers.rs"]
mod peers;

#[path = "distributed_factory_test_cases/peer_messages.rs"]
mod peer_messages;

#[path = "distributed_factory_test_cases/fake_cloud.rs"]
mod fake_cloud;

#[path = "distributed_factory_test_cases/test_cloud.rs"]
mod test_cloud;

#[path = "distributed_factory_test_cases/two_machine.rs"]
mod two_machine;

#[path = "distributed_factory_test_cases/epic_split.rs"]
mod epic_split;

#[path = "support/cloud_contract_replay.rs"]
mod cloud_contract_replay;

#[path = "distributed_factory_test_cases/contract_parity.rs"]
mod contract_parity;

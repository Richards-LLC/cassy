use super::*;
use std::sync::{Mutex, atomic::{AtomicBool, Ordering}};
use cas_store::PromptQueueStore;

fn recovery_fixture() -> Value {
    let value: Value = serde_json::from_str(include_str!("../../../../../tests/fixtures/coordination_contract/v1/deliveries--recover.json")).unwrap();
    let mut payload = value["cases"][0]["response"]["payload"].clone();
    payload["admissions"].as_array_mut().unwrap().truncate(1);
    payload
}
fn lookup_fixture() -> Value {
    let value: Value = serde_json::from_str(include_str!("../../../../../tests/fixtures/coordination_contract/v1/deliveries--lookup.json")).unwrap();
    value["cases"][0]["response"]["payload"].clone()
}
fn activation_fixture() -> Value {
    let value: Value = serde_json::from_str(include_str!("../../../../../tests/fixtures/coordination_contract/v1/bindings--activation.json")).unwrap();
    value["cases"][0]["response"]["payload"].clone()
}

struct DiscoveryCloud {
    db: std::path::PathBuf,
    lose_response: AtomicBool,
    foreign_target: bool,
    posts: Mutex<Vec<Value>>,
    lookups: Mutex<Vec<String>>,
}
impl CloudCoordinationPort for DiscoveryCloud {
    fn post(&self, path: &str, wire: &Value, timeout: Duration) -> Result<Value, RemoteError> {
        assert!(path.ends_with("/rebind"));
        assert!(!timeout.is_zero() && timeout<=CALL_REMOTE_BUDGET);
        let db = rusqlite::Connection::open(&self.db).unwrap();
        let stage: String = db.query_row("SELECT stage FROM external_work_operations WHERE request_id=?1",
            [wire["request_id"].as_str().unwrap()], |r|r.get(0)).unwrap();
        assert_eq!(stage,"dispatching", "intent precedes provider effect");
        self.posts.lock().unwrap().push(wire.clone());
        if self.lose_response.swap(false,Ordering::SeqCst) {
            Err(RemoteError::timeout("rebind committed, response lost"))
        } else {
            let mut value=lookup_fixture();
            value["receipt"]["request_id"]=wire["request_id"].clone();
            Ok(value)
        }
    }
    fn lookup(&self, request_id: &str, registration: &str, timeout: Duration) -> Result<Value, RemoteError> {
        assert_eq!(registration,"reg-supervisor-beta-08");
        assert!(!timeout.is_zero() && timeout<=CALL_REMOTE_BUDGET);
        self.lookups.lock().unwrap().push(request_id.into());
        if self.posts.lock().unwrap().is_empty() {
            return Ok(json!({"schema":CONTRACT_SCHEMA,"code":"not_found","reason":"unknown_request",
                "receipt":{"request_id":request_id,"next_action":"none"}}));
        }
        assert_eq!(request_id, self.posts.lock().unwrap()[0]["request_id"]);
        let mut value = lookup_fixture();
        value["receipt"]["request_id"] = json!(request_id);
        Ok(value)
    }
}
impl ActivityRecoveryPort for DiscoveryCloud {
    fn recover(&self, _: &OperationContext, _: Option<&str>, _: Duration) -> Result<Value, RemoteError> {
        let mut response=recovery_fixture();
        if !self.posts.lock().unwrap().is_empty() {
            // Model the real commit-then-timeout: subsequent discovery already
            // sees the new registration and no longer asks for a rebind.
            response["admissions"][0]=lookup_fixture()["admission"].clone();
        }
        Ok(response)
    }
    fn delivery(&self, _: &OperationContext, _: &str, _: Duration) -> Result<Value, RemoteError> {
        let mut value = lookup_fixture();
        if self.foreign_target { value["admission"]["target"]["project_id"] = json!("foreign-project"); }
        Ok(value)
    }
    fn activation(&self, _: &OperationContext, _: Duration) -> Result<Value, RemoteError> { Ok(activation_fixture()) }
}

#[test]
fn cas_c423_discovery_on_empty_machine_reconciles_same_request_and_restores_once() {
    let root = tempfile::tempdir().unwrap();
    let context = OperationContext {project_id:"github.com/richards-llc/cassy".into(),registration_id:"reg-supervisor-beta-08".into()};
    let destination = ReaderDestination {context:&context, factory_session:"successor-factory",recipient:"successor-supervisor"};
    let evidence = activation::validate_activation(&activation_fixture(),&context).unwrap();
    let cloud = DiscoveryCloud { db:root.path().join("cas.db"),lose_response:AtomicBool::new(true),foreign_target:false,posts:Mutex::new(vec![]),lookups:Mutex::new(vec![]) };
    let store = SqliteVioletAdmissionStore::open(root.path()).unwrap();
    let journal = SqliteRequestJournal::open(root.path()).unwrap();
    let report = recover_page(&cloud,&journal,&store,&destination,&evidence,None,Instant::now()+TOTAL_REMOTE_BUDGET);
    assert_eq!(report.errors.len(),1);
    assert_eq!(report.restored,0);
    let db=rusqlite::Connection::open(root.path().join("cas.db")).unwrap();
    db.execute("UPDATE external_work_operations SET next_retry_ms=0",[]).unwrap();
    drop(store); drop(journal);
    let store = SqliteVioletAdmissionStore::open(root.path()).unwrap();
    let journal = SqliteRequestJournal::open(root.path()).unwrap();
    let report = recover_page(&cloud,&journal,&store,&destination,&evidence,None,Instant::now()+TOTAL_REMOTE_BUDGET);
    assert!(report.errors.is_empty(),"{report:?}");
    assert_eq!(report.restored,1);
    assert_eq!(cloud.posts.lock().unwrap().len(),1);
    assert_eq!(cloud.lookups.lock().unwrap().len(),1);
    let again = recover_page(&cloud,&journal,&store,&destination,&evidence,None,Instant::now()+TOTAL_REMOTE_BUDGET);
    assert!(again.errors.is_empty(),"{again:?}");
    assert!(store.pending_acknowledgements(&context.registration_id,&context.project_id).unwrap().is_empty());
    let queue = cas_store::SqlitePromptQueueStore::open(root.path()).unwrap();
    assert_eq!(queue.peek_unseen_for_recipient(destination.recipient,Some(destination.factory_session),10).unwrap().len(),1);
    assert_eq!(cloud.posts.lock().unwrap().len(),1);
}

#[test]
fn cas_c423_discovery_refuses_foreign_lookup_target_before_sqlite_restore() {
    let root = tempfile::tempdir().unwrap();
    let context = OperationContext {project_id:"github.com/richards-llc/cassy".into(),registration_id:"reg-supervisor-beta-08".into()};
    let destination = ReaderDestination {context:&context, factory_session:"successor-factory",recipient:"successor-supervisor"};
    let evidence = activation::validate_activation(&activation_fixture(),&context).unwrap();
    let cloud = DiscoveryCloud { db:root.path().join("cas.db"),lose_response:AtomicBool::new(false),foreign_target:true,posts:Mutex::new(vec![]),lookups:Mutex::new(vec![]) };
    let store = SqliteVioletAdmissionStore::open(root.path()).unwrap();
    let journal = SqliteRequestJournal::open(root.path()).unwrap();
    let report = recover_page(&cloud,&journal,&store,&destination,&evidence,None,Instant::now()+TOTAL_REMOTE_BUDGET);
    assert_eq!(report.restored,0);
    assert!(report.errors.iter().any(|e|e.contains("project")),"{report:?}");
    let db = rusqlite::Connection::open(root.path().join("cas.db")).unwrap();
    let count: i64 = db.query_row("SELECT COUNT(*) FROM violet_admissions",[],|r|r.get(0)).unwrap();
    assert_eq!(count,0);
}

#[test]
fn cas_c423_discovery_crash_before_post_retries_original_id_after_lookup() {
    let root=tempfile::tempdir().unwrap();
    let context=OperationContext {project_id:"github.com/richards-llc/cassy".into(),registration_id:"reg-supervisor-beta-08".into()};
    let destination=ReaderDestination {context:&context,factory_session:"successor-factory",recipient:"successor-supervisor"};
    let evidence=activation::validate_activation(&activation_fixture(),&context).unwrap();
    let cloud=DiscoveryCloud {db:root.path().join("cas.db"),lose_response:AtomicBool::new(false),foreign_target:false,posts:Mutex::new(vec![]),lookups:Mutex::new(vec![])};
    let store=SqliteVioletAdmissionStore::open(root.path()).unwrap();
    let journal=SqliteRequestJournal::open(root.path()).unwrap();
    let request_id="00000071-2222-4222-8222-000000000071";
    let d=recovery_fixture()["admissions"][0]["delivery"].clone();
    let actor=json!({"registration_id":context.registration_id});
    let wire=json!({"schema":CONTRACT_SCHEMA,"request_id":request_id,"actor":actor,"expected_delivery_generation":1});
    let logical=json!({"actor":actor,"project_id":context.project_id,"wire":wire,"delivery_id":d["id"],
        "occurrence_id":d["occurrence_id"],"reader_slot_id":d["reader_slot_id"],
        "factory_session":destination.factory_session,"recipient":destination.recipient});
    journal.prepare(request_id,"delivery_rebind",&logical).unwrap();
    journal.set_wire_request(request_id,&wire).unwrap();
    journal.begin_attempt(request_id).unwrap(); // Crash after journal, before HTTP.
    for foreign in [OperationContext {project_id:context.project_id.clone(),registration_id:"foreign-registration".into()},
        OperationContext {project_id:"foreign-project".into(),registration_id:context.registration_id.clone()}] {
        assert!(!journal.has_unapplied_rebind(&foreign).unwrap());
        assert!(journal.lease_rebind_due(chrono::Utc::now().timestamp_millis(),&foreign).unwrap().is_empty());
    }
    let report=recover_page(&cloud,&journal,&store,&destination,&evidence,None,Instant::now()+TOTAL_REMOTE_BUDGET);
    assert!(report.errors.is_empty(),"{report:?}");
    assert_eq!(report.restored,1);
    assert_eq!(*cloud.lookups.lock().unwrap(),vec![request_id.to_string()]);
    assert_eq!(cloud.posts.lock().unwrap()[0]["request_id"],request_id);
    assert!(!journal.has_unapplied_rebind(&context).unwrap());
}

#[test]
fn cas_c423_discovery_terminal_receipt_retries_only_sqlite_apply_after_failure() {
    let root=tempfile::tempdir().unwrap();
    let context=OperationContext {project_id:"github.com/richards-llc/cassy".into(),registration_id:"reg-supervisor-beta-08".into()};
    let destination=ReaderDestination {context:&context,factory_session:"successor-factory",recipient:"successor-supervisor"};
    let evidence=activation::validate_activation(&activation_fixture(),&context).unwrap();
    let cloud=DiscoveryCloud {db:root.path().join("cas.db"),lose_response:AtomicBool::new(false),foreign_target:false,posts:Mutex::new(vec![]),lookups:Mutex::new(vec![])};
    let store=SqliteVioletAdmissionStore::open(root.path()).unwrap();
    let journal=SqliteRequestJournal::open(root.path()).unwrap();
    let db=rusqlite::Connection::open(&cloud.db).unwrap();
    db.execute_batch("CREATE TRIGGER fail_apply BEFORE INSERT ON violet_admission_cursors
        BEGIN SELECT RAISE(ABORT,'restore cursor failed'); END;").unwrap();
    let report=recover_page(&cloud,&journal,&store,&destination,&evidence,None,Instant::now()+TOTAL_REMOTE_BUDGET);
    assert!(report.errors.iter().any(|e|e.contains("restore cursor failed")),"{report:?}");
    assert_eq!(report.restored,0);
    let count:i64=db.query_row("SELECT COUNT(*) FROM violet_admissions",[],|r|r.get(0)).unwrap();
    assert_eq!(count,0);
    db.execute_batch("DROP TRIGGER fail_apply").unwrap();
    drop(journal);drop(store);
    let journal=SqliteRequestJournal::open(root.path()).unwrap();
    let store=SqliteVioletAdmissionStore::open(root.path()).unwrap();
    let report=recover_page(&cloud,&journal,&store,&destination,&evidence,None,Instant::now()+TOTAL_REMOTE_BUDGET);
    assert!(report.errors.is_empty(),"{report:?}");
    assert_eq!(report.restored,1);
    assert_eq!(cloud.posts.lock().unwrap().len(),1);
    assert!(cloud.lookups.lock().unwrap().is_empty(),"known terminal remote outcome needs only local apply");
}

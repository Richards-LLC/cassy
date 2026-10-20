use super::*;
use std::{collections::HashMap, sync::Mutex};
use super::super::external_work::RemoteError;

struct Authority { requests: Mutex<Vec<Value>>, capable: bool, admission: Value }
impl CloudCoordinationPort for Authority {
    fn get(&self, path: &str, _query: &[(&str, &str)], _timeout: Duration) -> Result<Value, RemoteError> {
        assert!(path.ends_with("/capabilities"));
        Ok(json!({"schema":SCHEMA,"atomic_reservations":self.capable,"violet_settlement":self.capable}))
    }
    fn post(&self, path: &str, payload: &Value, _timeout: Duration) -> Result<Value, RemoteError> {
        assert_eq!(path, format!("{BASE}/reserve"));
        self.requests.lock().unwrap().push(payload.clone());
        Ok(self.admission.clone())
    }
    fn lookup(&self, _: &str, _: &str, _: Duration) -> Result<Value, RemoteError> { unreachable!() }
}

fn configured(root: &Path) {
    let mut config = Config::default();
    config.set("slack.stakeholder_contacts", r#"[{"workspace_id":"T123","user_id":"U123","minimum_interval_secs":3600,"mode":"refuse"}]"#).unwrap();
    assert!(config.get("slack.stakeholder_contacts").unwrap().contains("3600"));
    std::fs::write(root.join("config.toml"), toml::to_string(&config).unwrap()).unwrap();
}

fn caller(id: &str) -> ProxyCaller {
    ProxyCaller { agent_id:id.into(), session_id:id.into(), role:crate::types::AgentRole::Supervisor,
        factory_session:None, active_task_ids:Vec::new() }
}

#[tokio::test]
async fn stakeholder_shared_window_refuses_second_supervisor_before_dispatch() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let detail = json!({"last_contact_at":"2026-10-20T16:23:00Z","last_poster":"peer-on-prowl",
        "permalink":"https://example.slack.com/archives/C123/p1792513380000000","next_allowed_at":"2026-10-20T17:23:00Z"});
    let cloud = Arc::new(Authority { capable:true, requests:Mutex::new(Vec::new()),
        admission:json!({"schema":SCHEMA,"code":"contact_too_recent","message":"Contacted at 16:23 by peer-on-prowl; next 17:23; https://example.slack.com/archives/C123/p1792513380000000","detail":detail}) });
    for (root, who) in [(first.path(), "machine-one"), (second.path(), "machine-two")] {
        configured(root);
        let service = StakeholderContacts { root:root.into(), cloud:Some(cloud.clone()), project_id:Some("shared-project".into()) };
        let engine = cmcp_core::ProxyEngine::from_configs(HashMap::new()).await.unwrap();
        engine.set_call_interceptor(Arc::new(service)).await;
        // A missing upstream makes an accidental forward fail the test.
        for args in [json!({"kind":"message","text":"<@U123> review","channel":"dev","request_id":"same-post"}),
            json!({"kind":"thread","text":"<@U123> review","channel":"dev","idempotency_key":"same-thread","replies":[]}),
            json!({"kind":"file","initial_comment":"<@U123> review","channel":"dev","file":{}}),
            json!({"kind":"file_external","action":"complete","initial_comment":"<@U123> review","channel":"dev","file_id":"F123"})] {
            let result = engine.call_tool(&caller(who), "violet", "violet_post", args.as_object().cloned()).await.unwrap();
            let receipt = receipt_payload(&result).unwrap();
            assert_eq!(receipt["error"]["code"], "contact_too_recent");
            assert_eq!(receipt["error"]["detail"], detail);
        }
        engine.shutdown().await;
    }
    let requests = cloud.requests.lock().unwrap();
    assert_eq!(requests.len(), 8);
    assert_eq!(requests[0]["registration_id"], "machine-one");
    assert_eq!(requests[4]["registration_id"], "machine-two");
    assert_eq!(requests[0]["policies"][0]["minimum_interval_secs"], 3600);
    assert_eq!(requests[0]["payload_sha256"].as_str().unwrap().len(), 64);
}

#[tokio::test]
async fn stakeholder_stamp_uses_root_receipt_and_degraded_warning_shows_local_time() {
    let root = tempfile::tempdir().unwrap();
    configured(root.path());
    let cloud = Arc::new(Authority { capable:false, requests:Mutex::new(Vec::new()), admission:Value::Null });
    let service = Arc::new(StakeholderContacts { root:root.path().into(), cloud:Some(cloud.clone()), project_id:Some("shared-project".into()) });
    let script = root.path().join("violet.py");
    std::fs::write(&script, r#"import json,sys
for line in sys.stdin:
 m=json.loads(line); i=m.get('id'); method=m.get('method')
 if i is None: continue
 if method=='initialize':
  r={'protocolVersion':m['params']['protocolVersion'],'capabilities':{'tools':{}},'serverInfo':{'name':'violet-double','version':'1'}}
 elif method=='tools/list':
  r={'tools':[{'name':'violet_post','inputSchema':{'type':'object'}}]}
 elif method=='tools/call':
  a=m['params']['arguments']
  if a.get('action')=='begin': p={'ok':True,'upload_url':'https://example.invalid/upload'}
  elif a.get('kind')=='thread': p={'ok':False,'posted':[{'index':0,'message_id':'1792513380.123456','permalink':'https://example.slack.com/root'}],'failed_index':1}
  else: p={'ok':False,'error':{'code':'refused','message':'no dispatch'}}
  r={'content':[{'type':'text','text':json.dumps(p)}],'isError':not p['ok']}
 else: r={}
 print(json.dumps({'jsonrpc':'2.0','id':i,'result':r}),flush=True)
"#).unwrap();
    let config = cmcp_core::config::ServerConfig::Stdio { command:"python3".into(),
        args:vec!["-I".into(), script.to_string_lossy().into()], env:HashMap::new() };
    let engine = cmcp_core::ProxyEngine::from_configs(HashMap::from([("violet".into(),config)])).await.unwrap();
    engine.set_call_interceptor(service.clone()).await;
    let who = caller("poster-on-soundwave");
    let call = |args: Value| async { engine.call_tool(&who, "violet", "violet_post", args.as_object().cloned()).await.unwrap() };
    call(json!({"kind":"file_external","action":"begin","initial_comment":"<@U123>","channel":"dev"})).await;
    call(json!({"kind":"message","text":"<@U123>","channel":"dev"})).await;
    assert!(service.status()["contacts"][0]["last_contact_at"].is_null(), "staging and refusals never stamp");
    let result = call(json!({"kind":"thread","text":"<@U123>","channel":"dev","replies":["fail"]})).await;
    assert!(serde_json::to_string(&result).unwrap().contains("shared exclusion is unavailable"));
    let status = service.status();
    assert_eq!(status["state"], "local_warning");
    let contact = &status["contacts"][0];
    assert_eq!(contact["last_contact_at"], "2026-10-20T16:23:00.123456+00:00");
    assert_eq!(contact["next_allowed_at"], "2026-10-20T17:23:00.123456+00:00");
    assert_eq!(contact["last_poster"], "poster-on-soundwave");
    let rendered = render_status(&status).join("\n");
    assert!(rendered.contains("local_warning") && rendered.contains("17:23:00") && rendered.contains("https://example.slack.com/root"));
    let next = call(json!({"kind":"message","text":"<@U123>","channel":"dev"})).await;
    let next = serde_json::to_string(&next).unwrap();
    assert!(next.contains("16:23:00") && next.contains("poster-on-soundwave"));
    assert!(cloud.requests.lock().unwrap().is_empty(), "no reserve calls without capability");
    engine.shutdown().await;
}

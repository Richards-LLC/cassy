use super::super::external_work::RemoteError;
use super::*;
use std::{collections::HashMap, sync::Mutex};

struct Authority {
    requests: Mutex<Vec<Value>>,
    capable: bool,
    admission: Value,
}
impl CloudCoordinationPort for Authority {
    fn get(
        &self,
        path: &str,
        _query: &[(&str, &str)],
        _timeout: Duration,
    ) -> Result<Value, RemoteError> {
        if path == BASE {
            return Ok(
                json!({"schema":SCHEMA,"code":"completed","observed_at":"2026-10-20T17:00:00Z","contacts":[self.admission["detail"]]}),
            );
        }
        assert!(path.ends_with("/capabilities"));
        Ok(
            json!({"schema":SCHEMA,"atomic_reservations":self.capable,"violet_settlement":self.capable}),
        )
    }
    fn post(&self, path: &str, payload: &Value, _timeout: Duration) -> Result<Value, RemoteError> {
        assert_eq!(path, format!("{BASE}/reserve"));
        self.requests.lock().unwrap().push(payload.clone());
        Ok(self.admission.clone())
    }
    fn lookup(&self, _: &str, _: &str, _: Duration) -> Result<Value, RemoteError> {
        unreachable!()
    }
}

fn configured(root: &Path) {
    let mut config = Config::default();
    config.set("slack.stakeholder_contacts", r#"[{"workspace_id":"T123","user_id":"U123","minimum_interval_secs":3600,"mode":"refuse"}]"#).unwrap();
    assert!(
        config
            .get("slack.stakeholder_contacts")
            .unwrap()
            .contains("3600")
    );
    assert!(
        config
            .list()
            .iter()
            .any(|(key, _)| key == "slack.stakeholder_contacts")
    );
    assert!(
        config
            .set(
                "slack.stakeholder_contacts",
                r#"[{"workspace_id":"T123","user_id":"U123","minimum_interval_secs":0}]"#
            )
            .is_err()
    );
    std::fs::write(root.join("config.toml"), toml::to_string(&config).unwrap()).unwrap();
}

fn caller(id: &str) -> ProxyCaller {
    ProxyCaller {
        agent_id: id.into(),
        session_id: id.into(),
        role: crate::types::AgentRole::Supervisor,
        factory_session: None,
        active_task_ids: Vec::new(),
    }
}

#[tokio::test]
async fn stakeholder_shared_window_refuses_second_supervisor_before_dispatch() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let detail = json!({"last_contact_at":"2026-10-20T16:23:00Z","last_poster":"peer-on-prowl",
        "permalink":"https://example.slack.com/archives/C123/p1792513380000000","next_allowed_at":"2026-10-20T17:23:00Z"});
    let cloud = Arc::new(Authority {
        capable: true,
        requests: Mutex::new(Vec::new()),
        admission: json!({"schema":SCHEMA,"code":"contact_too_recent","message":"Contacted at 16:23 by peer-on-prowl; next 17:23; https://example.slack.com/archives/C123/p1792513380000000","detail":detail}),
    });
    for (root, who) in [
        (first.path(), "machine-one"),
        (second.path(), "machine-two"),
    ] {
        configured(root);
        let service = StakeholderContacts {
            root: root.into(),
            cloud: Some(cloud.clone()),
            project_id: Some("shared-project".into()),
        };
        let status = service.status();
        assert_eq!(status["state"], "shared");
        assert_eq!(
            status["contacts"][0]["last_contact_at"],
            detail["last_contact_at"]
        );
        let engine = cmcp_core::ProxyEngine::from_configs(HashMap::new())
            .await
            .unwrap();
        engine.set_call_interceptor(Arc::new(service)).await;
        // A missing upstream makes an accidental forward fail the test.
        for args in [
            json!({"kind":"message","text":"<@U123> review","reply_to":"","channel":"dev","request_id":"10000000-0000-4000-8000-000000000001"}),
            json!({"kind":"thread","text":"<@U123> review","channel":"dev","idempotency_key":"same-thread","replies":[]}),
            json!({"kind":"file","initial_comment":"<@U123> review","channel":"dev","file":{}}),
            json!({"kind":"file_external","step":"complete","initial_comment":"<@U123> review","channel":"dev","file_id":"F123"}),
        ] {
            let result = engine
                .call_tool(
                    &caller(who),
                    "violet",
                    "violet_post",
                    args.as_object().cloned(),
                )
                .await
                .unwrap();
            let receipt = receipt_payload(&result).unwrap();
            assert_eq!(receipt["error"]["code"], "contact_too_recent");
            for key in [
                "last_contact_at",
                "last_poster",
                "permalink",
                "next_allowed_at",
            ] {
                assert_eq!(receipt["error"]["detail"][key], detail[key]);
            }
        }
        engine.shutdown().await;
    }
    let requests = cloud.requests.lock().unwrap();
    assert_eq!(requests.len(), 8);
    assert_eq!(requests[0]["registration_id"], "machine-one");
    assert_eq!(requests[4]["registration_id"], "machine-two");
    assert_eq!(requests[0]["policies"][0]["minimum_interval_secs"], 3600);
    assert_eq!(requests[0]["payload_sha256"].as_str().unwrap().len(), 64);
    drop(requests);

    let grant = Arc::new(Authority {
        capable: true,
        requests: Mutex::new(Vec::new()),
        admission: json!({"schema":SCHEMA,"code":"acquired","authorization":{"operation_id":"op","token":"test-bound-grant"},"warnings":["Previous contact: 16:23 by peer-on-prowl; https://example.slack.com/root"]}),
    });
    let service = StakeholderContacts {
        root: first.path().into(),
        cloud: Some(grant.clone()),
        project_id: Some("shared-project".into()),
    };
    let args = json!({"kind":"message","text":"<@U123> new","channel":"dev","request_id":"10000000-0000-4000-8000-000000000002","contact_authorization":{"token":"forged"}});
    let prepared = service
        .prepare_post(&caller("machine-one"), args.as_object().cloned())
        .unwrap();
    assert_eq!(
        prepared.arguments.unwrap()["contact_authorization"]["token"],
        "test-bound-grant"
    );
    assert!(prepared.warning.unwrap().contains("peer-on-prowl"));
    assert!(
        prepared
            .failure_context
            .unwrap()
            .contains("10000000-0000-4000-8000-000000000002")
    );
    assert!(
        SqliteRequestJournal::read_stakeholder_contact_evidence(first.path())
            .unwrap()
            .is_empty(),
        "a grant is never a publication stamp"
    );
    let mut changed = args;
    changed["text"] = json!("<@U123> changed");
    let response = service
        .prepare_post(&caller("machine-one"), changed.as_object().cloned())
        .unwrap()
        .response
        .unwrap();
    assert_eq!(
        receipt_payload(&serde_json::to_value(response).unwrap()).unwrap()["error"]["code"],
        "idempotency_conflict"
    );
    let pending = StakeholderContacts {
        root: first.path().into(),
        project_id: Some("shared-project".into()),
        cloud: Some(Arc::new(Authority {
            capable: true,
            requests: Mutex::new(Vec::new()),
            admission: json!({"schema":SCHEMA,"code":"outcome_unknown","message":"Reconcile original dispatch"}),
        })),
    };
    let response = pending.prepare_post(&caller("machine-one"), json!({"kind":"message","text":"<@U123>","channel":"dev","request_id":"10000000-0000-4000-8000-000000000003"}).as_object().cloned()).unwrap().response.unwrap();
    assert_eq!(
        receipt_payload(&serde_json::to_value(response).unwrap()).unwrap()["error"]["code"],
        "outcome_unknown"
    );
    let journal = SqliteRequestJournal::open(first.path()).unwrap();
    assert!(
        !journal.has_due(i64::MAX / 2).unwrap(),
        "thread reconciliation must not pick contact operations"
    );
    assert!(
        journal
            .lease_due(
                1,
                chrono::Utc::now().timestamp_millis(),
                Duration::from_secs(1)
            )
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn stakeholder_stamp_uses_root_receipt_and_degraded_warning_shows_local_time() {
    let root = tempfile::tempdir().unwrap();
    configured(root.path());
    let cloud = Arc::new(Authority {
        capable: false,
        requests: Mutex::new(Vec::new()),
        admission: Value::Null,
    });
    let service = Arc::new(StakeholderContacts {
        root: root.path().into(),
        cloud: Some(cloud.clone()),
        project_id: Some("shared-project".into()),
    });
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
  if a.get('step')=='begin': p={'ok':True,'upload_url':'https://example.invalid/upload'}
  elif a.get('kind')=='thread': p={'ok':False,'posted':[{'index':0,'message_id':'1792513380.123456','permalink':'https://example.slack.com/root'}],'failed_index':1}
  else: p={'ok':False,'error':{'code':'refused','message':'no dispatch'}}
  r={'content':[],'structuredContent':p,'isError':not p['ok']} if a.get('kind')=='thread' else {'content':[{'type':'text','text':json.dumps(p)}],'isError':not p['ok']}
 else: r={}
 print(json.dumps({'jsonrpc':'2.0','id':i,'result':r}),flush=True)
"#).unwrap();
    let config = cmcp_core::config::ServerConfig::Stdio {
        command: "python3".into(),
        args: vec!["-I".into(), script.to_string_lossy().into()],
        env: HashMap::new(),
    };
    let engine = cmcp_core::ProxyEngine::from_configs(HashMap::from([("violet".into(), config)]))
        .await
        .unwrap();
    engine.set_call_interceptor(service.clone()).await;
    let who = caller("poster-on-soundwave");
    let call = |args: Value| {
        let engine = &engine;
        let who = &who;
        async move {
            engine
                .call_tool(who, "violet", "violet_post", args.as_object().cloned())
                .await
                .unwrap()
        }
    };
    call(
        json!({"kind":"file_external","step":"begin","initial_comment":"<@U123>","channel":"dev"}),
    )
    .await;
    call(json!({"kind":"message","text":"<@U123>","channel":"dev"})).await;
    call(json!({"kind":"message","text":"<@U123>","channel":"dev","reply_to":"1792513000.000001"}))
        .await;
    assert!(
        service.status()["contacts"][0]["last_contact_at"].is_null(),
        "staging and refusals never stamp"
    );
    let result = engine.execute(&who, &json!({"server":"violet","tool":"violet_post","args":{"kind":"thread","text":"<@U123>","channel":"dev","replies":["fail"]}}).to_string(), None).await.unwrap();
    assert!(
        result.text.contains("shared exclusion is unavailable")
            && result.text.contains("1792513380.123456")
    );
    let status = service.status();
    assert_eq!(status["state"], "local_warning");
    let contact = &status["contacts"][0];
    assert_eq!(
        contact["last_contact_at"],
        "2026-10-20T16:23:00.123456+00:00"
    );
    assert_eq!(
        contact["next_allowed_at"],
        "2026-10-20T17:23:00.123456+00:00"
    );
    assert_eq!(contact["last_poster"], "poster-on-soundwave");
    let rendered = render_status(&status).join("\n");
    assert!(
        rendered.contains("local_warning")
            && rendered.contains("17:23:00")
            && rendered.contains("https://example.slack.com/root")
    );
    let next = call(json!({"kind":"message","text":"<@U123>","channel":"dev"})).await;
    let next = serde_json::to_string(&next).unwrap();
    assert!(next.contains("16:23:00") && next.contains("poster-on-soundwave"));
    assert!(
        cloud.requests.lock().unwrap().is_empty(),
        "no reserve calls without capability"
    );
    let journal = SqliteRequestJournal::open(root.path()).unwrap();
    assert!(!journal.has_due(i64::MAX / 2).unwrap());
    engine.shutdown().await;
}

#[tokio::test]
async fn stakeholder_broken_config_does_not_refuse_posts_or_replies() {
    let root = tempfile::tempdir().unwrap();
    let service = StakeholderContacts {
        root: root.path().into(),
        cloud: None,
        project_id: None,
    };
    let who = caller("supervisor");
    for config in [
        "[slack",
        "[[slack.stakeholder_contacts]]\nworkspace_id='T123'\nuser_id='U123'\nminimum_interval_secs=0\n",
    ] {
        std::fs::write(root.path().join("config.toml"), config).unwrap();
        for (args, warning) in [
            (
                json!({"kind":"message","channel":"dev","text":"ordinary update"}),
                false,
            ),
            (
                json!({"kind":"message","channel":"dev","text":"<@U123> reply","reply_to":"1792513000.000001"}),
                false,
            ),
            (
                json!({"kind":"message","channel":"dev","text":"<@U123> root"}),
                true,
            ),
        ] {
            let prepared = service
                .prepare(&who, "violet", "violet_post", args.as_object().cloned())
                .await
                .unwrap();
            assert!(prepared.response.is_none(), "config error blocked {args}");
            assert_eq!(prepared.arguments.as_ref(), args.as_object());
            assert_eq!(prepared.warning.is_some(), warning, "{args}");
            if let Some(warning) = prepared.warning {
                assert!(warning.contains("configuration") && warning.contains("unknown"));
            }
        }
    }
}

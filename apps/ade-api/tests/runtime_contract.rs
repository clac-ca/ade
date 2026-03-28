use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{Arc, Mutex},
};

use ade_api::{
    readiness::{CreateReadinessControllerOptions, ReadinessController, ReadinessPhase},
    router::create_app,
    runtime::{
        ActiveConfigArtifact, EventBatch, InstalledConfigStatus, RunStatus, RuntimeEvent,
        RuntimeService, RuntimeStatus, SessionRuntime, TerminalStatus, UploadedFile,
    },
    state::AppState,
    unix_time_ms,
};
use async_trait::async_trait;
use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode},
};
use serde_json::{Value, json};
use tower::util::ServiceExt;

use ade_api::error::AppError;

struct FakeRuntimeState {
    download_files: HashMap<String, Vec<u8>>,
    ensure_calls: usize,
    event_batches: VecDeque<EventBatch>,
    install_calls: usize,
    poll_afters: Vec<u64>,
    reset_calls: usize,
    rpc_calls: Vec<(String, Value)>,
    status: RuntimeStatus,
    upload_calls: Vec<(String, Vec<u8>)>,
}

impl Default for FakeRuntimeState {
    fn default() -> Self {
        Self {
            download_files: HashMap::new(),
            ensure_calls: 0,
            event_batches: VecDeque::new(),
            install_calls: 0,
            poll_afters: Vec::new(),
            reset_calls: 0,
            rpc_calls: Vec::new(),
            status: runtime_status(),
            upload_calls: Vec::new(),
        }
    }
}

#[derive(Clone)]
struct FakeRuntime {
    state: Arc<Mutex<FakeRuntimeState>>,
}

impl FakeRuntime {
    fn new(state: FakeRuntimeState) -> Self {
        Self {
            state: Arc::new(Mutex::new(state)),
        }
    }
}

#[async_trait]
impl SessionRuntime for FakeRuntime {
    async fn ensure_session(&self, _: &str) -> Result<(), AppError> {
        self.state.lock().unwrap().ensure_calls += 1;
        Ok(())
    }

    async fn status(&self, _: &str) -> Result<RuntimeStatus, AppError> {
        Ok(self.state.lock().unwrap().status.clone())
    }

    async fn install_config(
        &self,
        _: &str,
        artifact: &ActiveConfigArtifact,
    ) -> Result<RuntimeStatus, AppError> {
        let mut state = self.state.lock().unwrap();
        state.install_calls += 1;
        state.status.installed_config = Some(InstalledConfigStatus {
            package_name: artifact.package_name.clone(),
            version: artifact.version.clone(),
            sha256: artifact.sha256.clone(),
            fingerprint: artifact.fingerprint.clone(),
        });
        Ok(state.status.clone())
    }

    async fn upload_file(
        &self,
        _: &str,
        relative_path: &str,
        content: Vec<u8>,
    ) -> Result<UploadedFile, AppError> {
        let mut state = self.state.lock().unwrap();
        state
            .upload_calls
            .push((relative_path.to_string(), content.clone()));
        state
            .download_files
            .insert(relative_path.to_string(), content.clone());
        Ok(UploadedFile {
            path: relative_path.to_string(),
            size: content.len(),
        })
    }

    async fn download_file(&self, _: &str, relative_path: &str) -> Result<Vec<u8>, AppError> {
        self.state
            .lock()
            .unwrap()
            .download_files
            .get(relative_path)
            .cloned()
            .ok_or_else(|| AppError::not_found(format!("Missing file: {relative_path}")))
    }

    async fn rpc(&self, _: &str, method: &str, params: Value) -> Result<Value, AppError> {
        self.state
            .lock()
            .unwrap()
            .rpc_calls
            .push((method.to_string(), params.clone()));
        Ok(json!({"ok": true, "method": method, "params": params}))
    }

    async fn poll_events(&self, _: &str, after: u64, _: u64) -> Result<EventBatch, AppError> {
        let mut state = self.state.lock().unwrap();
        state.poll_afters.push(after);
        if let Some(batch) = state.event_batches.pop_front() {
            return Ok(batch);
        }

        Err(AppError::internal("stop".to_string()))
    }

    async fn reset_session(&self, _: &str) -> Result<(), AppError> {
        self.state.lock().unwrap().reset_calls += 1;
        Ok(())
    }
}

fn ready_state() -> ReadinessController {
    let readiness = ReadinessController::new(CreateReadinessControllerOptions {
        database_ok: Some(true),
        last_checked_at: Some(unix_time_ms()),
        phase: Some(ReadinessPhase::Ready),
        ..CreateReadinessControllerOptions::default()
    });
    readiness.mark_ready();
    readiness
}

fn runtime_status() -> RuntimeStatus {
    RuntimeStatus {
        workspace: "/workspace".to_string(),
        installed_config: None,
        terminal: TerminalStatus {
            open: false,
            cwd: "/workspace".to_string(),
            cols: None,
            rows: None,
        },
        run: RunStatus {
            active: false,
            input_path: None,
            output_path: None,
            pid: None,
        },
        earliest_seq: 1,
        latest_seq: 1,
    }
}

fn active_config_artifact() -> ActiveConfigArtifact {
    ActiveConfigArtifact {
        package_name: "ade-config".to_string(),
        version: "0.1.0".to_string(),
        sha256: "abc123".to_string(),
        fingerprint: "ade-config@0.1.0:abc123".to_string(),
        wheel_path: PathBuf::from("/tmp/ade-config-test.whl"),
    }
}

fn app_with_runtime(runtime: FakeRuntime) -> axum::Router {
    let service = Arc::new(RuntimeService::new(
        Arc::new(runtime),
        active_config_artifact(),
        "cfg-test".to_string(),
    ));

    create_app(AppState {
        readiness: ready_state(),
        runtime: Some(service),
        web_root: None,
    })
}

async fn json_body(response: axum::response::Response) -> serde_json::Value {
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}

fn request(uri: &str, method: Method, body: Body) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .body(body)
        .unwrap()
}

#[tokio::test]
async fn ensure_route_installs_the_active_config_when_needed() {
    let fake_runtime = FakeRuntime::new(FakeRuntimeState {
        status: runtime_status(),
        ..FakeRuntimeState::default()
    });
    let state = fake_runtime.state.clone();
    let app = app_with_runtime(fake_runtime);

    let response = app
        .oneshot(request(
            "/api/runtime/session/ensure",
            Method::POST,
            Body::empty(),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let payload = json_body(response).await;
    assert_eq!(
        payload["status"]["installedConfig"]["fingerprint"],
        "ade-config@0.1.0:abc123"
    );
    assert_eq!(state.lock().unwrap().install_calls, 1);
}

#[tokio::test]
async fn terminal_open_route_calls_the_runtime_rpc() {
    let fake_runtime = FakeRuntime::new(FakeRuntimeState {
        status: RuntimeStatus {
            installed_config: Some(InstalledConfigStatus {
                package_name: "ade-config".to_string(),
                version: "0.1.0".to_string(),
                sha256: "abc123".to_string(),
                fingerprint: "ade-config@0.1.0:abc123".to_string(),
            }),
            ..runtime_status()
        },
        ..FakeRuntimeState::default()
    });
    let state = fake_runtime.state.clone();
    let app = app_with_runtime(fake_runtime);

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/runtime/terminal/open")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"rows":30,"cols":120}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let calls = &state.lock().unwrap().rpc_calls;
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "terminal.open");
    assert_eq!(calls[0].1, json!({"rows": 30, "cols": 120}));
}

#[tokio::test]
async fn events_route_resumes_from_last_event_id() {
    let fake_runtime = FakeRuntime::new(FakeRuntimeState {
        status: RuntimeStatus {
            installed_config: Some(InstalledConfigStatus {
                package_name: "ade-config".to_string(),
                version: "0.1.0".to_string(),
                sha256: "abc123".to_string(),
                fingerprint: "ade-config@0.1.0:abc123".to_string(),
            }),
            latest_seq: 6,
            ..runtime_status()
        },
        event_batches: VecDeque::from([EventBatch {
            needs_resync: false,
            status: RuntimeStatus {
                installed_config: Some(InstalledConfigStatus {
                    package_name: "ade-config".to_string(),
                    version: "0.1.0".to_string(),
                    sha256: "abc123".to_string(),
                    fingerprint: "ade-config@0.1.0:abc123".to_string(),
                }),
                latest_seq: 6,
                ..runtime_status()
            },
            events: vec![RuntimeEvent {
                seq: 6,
                time: 1,
                event_type: "run.log".to_string(),
                payload: json!({"message": "hello"}),
            }],
        }]),
        ..FakeRuntimeState::default()
    });
    let state = fake_runtime.state.clone();
    let app = app_with_runtime(fake_runtime);

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/runtime/events")
                .header("Last-Event-ID", "5")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(body.contains("id: 6"));
    assert!(body.contains("event: run.log"));
    assert_eq!(state.lock().unwrap().poll_afters.first().copied(), Some(5));
}

#[tokio::test]
async fn reset_route_calls_the_runtime_reset() {
    let fake_runtime = FakeRuntime::new(FakeRuntimeState {
        status: runtime_status(),
        ..FakeRuntimeState::default()
    });
    let state = fake_runtime.state.clone();
    let app = app_with_runtime(fake_runtime);

    let response = app
        .oneshot(request(
            "/api/runtime/session/reset",
            Method::POST,
            Body::empty(),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(state.lock().unwrap().reset_calls, 1);
}

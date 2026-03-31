use std::{
    collections::HashMap,
    collections::hash_map::Entry,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Stdio,
    sync::mpsc as std_mpsc,
    thread,
    time::Duration,
};

use ade_api::{
    runs::{RunPhase, RunValidationIssue},
    session_agent::{
        SessionAgentCommand, SessionAgentEvent, SessionArtifactAccess, WorkerId, WorkerKind,
    },
};
use clap::Parser;
use flate2::read::GzDecoder;
use futures_util::{SinkExt, StreamExt};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use reqwest::{Client, Method};
use serde::Deserialize;
use tar::Archive;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    sync::{mpsc, watch},
    time::Instant,
};
use tokio_tungstenite::{connect_async, tungstenite::Message};

const PROCESS_RESULT_PREFIX: &str = "__ADE_PROCESS_RESULT__=";
const ARTIFACT_REQUEST_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const ARTIFACT_REQUEST_TIMEOUT: Duration = Duration::from_secs(300);
const CONTROL_CHANNEL_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_SHELL_PATH: &str = "/bin/sh";
const PROCESSOR_CODE: &str = r#"
import json
import sys
from pathlib import Path
from ade_engine import load_config
from ade_engine.runner import process

PROCESS_RESULT_PREFIX = "__ADE_PROCESS_RESULT__="

input_path = Path(sys.argv[1])
output_dir = Path(sys.argv[2])
config = load_config("ade_config", name="ade-config")
result = process(config=config, input_path=input_path, output_dir=output_dir)
print(
    PROCESS_RESULT_PREFIX
    + json.dumps(
        {
            "localOutputPath": str(result.output_path),
            "validationIssues": [
                {
                    "rowIndex": issue.row_index,
                    "field": issue.field,
                    "message": issue.message,
                }
                for issue in result.validation_issues
            ],
        },
        separators=(",", ":"),
    ),
    flush=True,
)
"#;

#[derive(Parser)]
struct Args {
    #[arg(long)]
    config_file: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentConfig {
    bridge_url: String,
    idle_shutdown_seconds: u64,
}

#[derive(Clone, Debug)]
struct PreparedRuntime {
    python_executable_path: String,
}

enum WorkerHandle {
    Run(RunWorkerHandle),
    Shell(ShellWorkerHandle),
}

impl WorkerHandle {
    fn cancel(&self) {
        match self {
            Self::Run(worker) => worker.cancel(),
            Self::Shell(worker) => worker.close(),
        }
    }

    fn close(&self) {
        self.cancel();
    }

    fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        match self {
            Self::Shell(worker) => worker.resize(cols, rows),
            Self::Run(_) => Err("Run workers do not support PTY resize.".to_string()),
        }
    }

    fn write_input(&self, data: String) -> Result<(), String> {
        match self {
            Self::Shell(worker) => worker.write_input(data),
            Self::Run(_) => Err("Run workers do not accept terminal input.".to_string()),
        }
    }
}

struct ShellWorkerHandle {
    control_tx: std_mpsc::Sender<ShellControl>,
}

impl ShellWorkerHandle {
    fn close(&self) {
        let _ = self.control_tx.send(ShellControl::Close);
    }

    fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        self.control_tx
            .send(ShellControl::Resize { cols, rows })
            .map_err(|_| "Shell worker is unavailable.".to_string())
    }

    fn write_input(&self, data: String) -> Result<(), String> {
        self.control_tx
            .send(ShellControl::Input(data))
            .map_err(|_| "Shell worker is unavailable.".to_string())
    }
}

struct RunWorkerHandle {
    cancel_tx: watch::Sender<bool>,
}

impl RunWorkerHandle {
    fn cancel(&self) {
        let _ = self.cancel_tx.send(true);
    }
}

enum ShellControl {
    Close,
    Input(String),
    Resize { cols: u16, rows: u16 },
}

#[derive(Debug)]
enum InternalEvent {
    Session(SessionAgentEvent),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProcessResult {
    local_output_path: String,
    validation_issues: Vec<RunValidationIssue>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args = Args::parse();
    let config: AgentConfig = serde_json::from_str(&std::fs::read_to_string(args.config_file)?)?;
    run(config).await?;
    Ok(())
}

async fn run(config: AgentConfig) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (stream, _) = tokio::time::timeout(
        CONTROL_CHANNEL_CONNECT_TIMEOUT,
        connect_async(config.bridge_url.as_str()),
    )
    .await
    .map_err(|_| io_error("Timed out connecting the control channel.".to_string()))??;
    let (mut socket_tx, mut socket_rx) = stream.split();

    let (outgoing_tx, mut outgoing_rx) = mpsc::unbounded_channel::<SessionAgentEvent>();
    let (command_tx, mut command_rx) = mpsc::unbounded_channel::<SessionAgentCommand>();
    let (internal_tx, mut internal_rx) = mpsc::unbounded_channel::<InternalEvent>();

    let reader_outgoing = outgoing_tx.clone();
    let mut reader_task = tokio::spawn(async move {
        while let Some(message) = socket_rx.next().await {
            match message {
                Ok(Message::Text(text)) => match serde_json::from_str::<SessionAgentCommand>(&text)
                {
                    Ok(command) => {
                        if command_tx.send(command).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = reader_outgoing.send(SessionAgentEvent::Error {
                            message: format!("Invalid session-agent command: {error}"),
                            phase: None,
                            retriable: false,
                            worker_id: None,
                        });
                        break;
                    }
                },
                Ok(Message::Close(_)) => break,
                Ok(Message::Binary(_)) => {
                    let _ = reader_outgoing.send(SessionAgentEvent::Error {
                        message: "Binary session-agent commands are not supported.".to_string(),
                        phase: None,
                        retriable: false,
                        worker_id: None,
                    });
                    break;
                }
                Ok(Message::Ping(_)) | Ok(Message::Pong(_)) | Ok(Message::Frame(_)) => {}
                Err(error) => {
                    let _ = reader_outgoing.send(SessionAgentEvent::Error {
                        message: format!("Control channel read failed: {error}"),
                        phase: None,
                        retriable: true,
                        worker_id: None,
                    });
                    break;
                }
            }
        }
    });

    let mut writer_task = tokio::spawn(async move {
        while let Some(event) = outgoing_rx.recv().await {
            let payload = serde_json::to_string(&event).map_err(|error| {
                io_error(format!("Failed to encode session-agent event: {error}"))
            })?;
            socket_tx
                .send(Message::Text(payload.into()))
                .await
                .map_err(|error| {
                    io_error(format!("Failed to send a session-agent event: {error}"))
                })?;
        }
        socket_tx
            .close()
            .await
            .map_err(|error| io_error(format!("Failed to close the control channel: {error}")))
    });

    let _ = outgoing_tx.send(SessionAgentEvent::Ready);

    let mut prepared: Option<PreparedRuntime> = None;
    let mut workers: HashMap<WorkerId, WorkerHandle> = HashMap::new();
    let idle_timer = tokio::time::sleep(Duration::from_secs(365 * 24 * 60 * 60));
    tokio::pin!(idle_timer);
    let mut idle_armed = false;

    loop {
        tokio::select! {
            command = command_rx.recv() => {
                let Some(command) = command else {
                    break;
                };

                match command {
                    SessionAgentCommand::Prepare {
                        config_package_name,
                        config_version,
                        config_wheel_path,
                        engine_package_name,
                        engine_version,
                        engine_wheel_path,
                        python_executable_path,
                        python_home_path,
                        python_toolchain_path,
                        python_toolchain_version,
                    } => {
                        match prepare_runtime(PrepareRequest {
                                config_package_name,
                                config_version: config_version.clone(),
                                config_wheel_path,
                                engine_package_name,
                                engine_version: engine_version.clone(),
                                engine_wheel_path,
                                python_executable_path: python_executable_path.clone(),
                                python_home_path: python_home_path.clone(),
                                python_toolchain_path,
                            },
                            outgoing_tx.clone(),
                        )
                        .await
                        {
                            Ok(runtime) => {
                                prepared = Some(runtime);
                                let _ = outgoing_tx.send(SessionAgentEvent::Prepared {
                                    config_version,
                                    engine_version,
                                    python_toolchain_version,
                                });
                            }
                            Err((phase, message, retriable)) => {
                                let _ = outgoing_tx.send(SessionAgentEvent::Error {
                                    message,
                                    phase: Some(phase),
                                    retriable,
                                    worker_id: None,
                                });
                            }
                        }
                    }
                    SessionAgentCommand::StartShell { cols, cwd, rows, worker_id } => {
                        match workers.entry(worker_id.clone()) {
                            Entry::Occupied(_) => {
                                let _ = outgoing_tx.send(SessionAgentEvent::Error {
                                    message: format!("Worker '{}' already exists.", worker_id.0),
                                    phase: None,
                                    retriable: false,
                                    worker_id: Some(worker_id),
                                });
                            }
                            Entry::Vacant(entry) => match start_shell_worker(
                                worker_id.clone(),
                                cwd,
                                rows,
                                cols,
                                internal_tx.clone(),
                            ) {
                                Ok(worker) => {
                                    entry.insert(WorkerHandle::Shell(worker));
                                }
                                Err(message) => {
                                    let _ = outgoing_tx.send(SessionAgentEvent::Error {
                                        message,
                                        phase: None,
                                        retriable: true,
                                        worker_id: Some(worker_id),
                                    });
                                }
                            },
                        }
                    }
                    SessionAgentCommand::WriteInput { data, worker_id } => {
                        if let Some(worker) = workers.get(&worker_id)
                            && let Err(message) = worker.write_input(data)
                        {
                            let _ = outgoing_tx.send(SessionAgentEvent::Error {
                                message,
                                phase: None,
                                retriable: true,
                                worker_id: Some(worker_id),
                            });
                        }
                    }
                    SessionAgentCommand::ResizePty { cols, rows, worker_id } => {
                        if let Some(worker) = workers.get(&worker_id)
                            && let Err(message) = worker.resize(cols, rows)
                        {
                            let _ = outgoing_tx.send(SessionAgentEvent::Error {
                                message,
                                phase: None,
                                retriable: false,
                                worker_id: Some(worker_id),
                            });
                        }
                    }
                    SessionAgentCommand::CloseWorker { worker_id } | SessionAgentCommand::CancelWorker { worker_id } => {
                        if let Some(worker) = workers.get(&worker_id) {
                            worker.close();
                        }
                    }
                    SessionAgentCommand::StartRun {
                        input_download,
                        local_input_path,
                        local_output_dir,
                        output_path,
                        output_upload,
                        timeout_in_seconds,
                        worker_id,
                    } => {
                        let Some(runtime) = prepared.clone() else {
                            let _ = outgoing_tx.send(SessionAgentEvent::Error {
                                message: "Scope session is not prepared.".to_string(),
                                phase: Some(RunPhase::InstallPackages),
                                retriable: true,
                                worker_id: Some(worker_id),
                            });
                            continue;
                        };
                        if workers.contains_key(&worker_id) {
                            let _ = outgoing_tx.send(SessionAgentEvent::Error {
                                message: format!("Worker '{}' already exists.", worker_id.0),
                                phase: None,
                                retriable: false,
                                worker_id: Some(worker_id),
                            });
                            continue;
                        }

                        let worker = start_run_worker(
                            runtime,
                            StartRunRequest {
                                input_download,
                                local_input_path,
                                local_output_dir,
                                output_path,
                                output_upload,
                                timeout_in_seconds,
                                worker_id: worker_id.clone(),
                            },
                            internal_tx.clone(),
                        );
                        workers.insert(worker_id, WorkerHandle::Run(worker));
                    }
                    SessionAgentCommand::Shutdown => break,
                }

                refresh_idle_timer(
                    &config,
                    workers.is_empty(),
                    &mut idle_armed,
                    idle_timer.as_mut(),
                );
            }
            event = internal_rx.recv() => {
                let Some(InternalEvent::Session(event)) = event else {
                    break;
                };

                if let SessionAgentEvent::WorkerExit { worker_id, .. } = &event {
                    workers.remove(worker_id);
                }

                let _ = outgoing_tx.send(event);
                refresh_idle_timer(
                    &config,
                    workers.is_empty(),
                    &mut idle_armed,
                    idle_timer.as_mut(),
                );
            }
            result = &mut reader_task => {
                if let Err(error) = result {
                    let _ = outgoing_tx.send(SessionAgentEvent::Error {
                        message: format!("Control channel reader task failed: {error}"),
                        phase: None,
                        retriable: true,
                        worker_id: None,
                    });
                }
                break;
            }
            result = &mut writer_task => {
                if let Err(error) = result {
                    let _ = outgoing_tx.send(SessionAgentEvent::Error {
                        message: format!("Control channel writer task failed: {error}"),
                        phase: None,
                        retriable: true,
                        worker_id: None,
                    });
                }
                break;
            }
            _ = &mut idle_timer, if idle_armed => {
                break;
            }
        }
    }

    for worker in workers.values() {
        worker.close();
    }
    drop(outgoing_tx);
    if !reader_task.is_finished() {
        reader_task.abort();
    }
    if !writer_task.is_finished() {
        writer_task.abort();
    }
    let _ = reader_task.await;
    let _ = writer_task.await;
    Ok(())
}

fn refresh_idle_timer(
    config: &AgentConfig,
    empty: bool,
    idle_armed: &mut bool,
    mut idle_timer: std::pin::Pin<&mut tokio::time::Sleep>,
) {
    if empty {
        *idle_armed = true;
        idle_timer
            .as_mut()
            .reset(Instant::now() + Duration::from_secs(config.idle_shutdown_seconds));
    } else {
        *idle_armed = false;
        idle_timer
            .as_mut()
            .reset(Instant::now() + Duration::from_secs(365 * 24 * 60 * 60));
    }
}

#[derive(Clone)]
struct PrepareRequest {
    config_package_name: String,
    config_version: String,
    config_wheel_path: String,
    engine_package_name: String,
    engine_version: String,
    engine_wheel_path: String,
    python_executable_path: String,
    python_home_path: String,
    python_toolchain_path: String,
}

async fn prepare_runtime(
    request: PrepareRequest,
    outgoing_tx: mpsc::UnboundedSender<SessionAgentEvent>,
) -> Result<PreparedRuntime, (RunPhase, String, bool)> {
    if runtime_matches(&request)
        .await
        .map_err(|error| (RunPhase::InstallPackages, error, true))?
    {
        let _ = outgoing_tx.send(SessionAgentEvent::Log {
            phase: Some(RunPhase::InstallPackages),
            level: "info".to_string(),
            message: "Python toolchain and ADE packages are already prepared.".to_string(),
            worker_id: None,
        });
        return Ok(PreparedRuntime {
            python_executable_path: request.python_executable_path,
        });
    }

    ensure_python_toolchain(&request)
        .await
        .map_err(|error| (RunPhase::InstallPackages, error, true))?;
    install_python_package(
        &request.python_executable_path,
        &request.engine_package_name,
        &request.engine_version,
        &request.engine_wheel_path,
        &["--force-reinstall"],
        outgoing_tx.clone(),
    )
    .await
    .map_err(|error| (RunPhase::InstallPackages, error, true))?;
    install_python_package(
        &request.python_executable_path,
        &request.config_package_name,
        &request.config_version,
        &request.config_wheel_path,
        &["--no-deps", "--force-reinstall"],
        outgoing_tx,
    )
    .await
    .map_err(|error| (RunPhase::InstallPackages, error, true))?;

    Ok(PreparedRuntime {
        python_executable_path: request.python_executable_path,
    })
}

async fn runtime_matches(request: &PrepareRequest) -> Result<bool, String> {
    if !tokio::fs::try_exists(&request.python_executable_path)
        .await
        .map_err(|error| format!("Failed to check the prepared Python runtime: {error}"))?
    {
        return Ok(false);
    }

    let engine_version = query_distribution_version(
        &request.python_executable_path,
        &request.engine_package_name,
    )
    .await?;
    let config_version = query_distribution_version(
        &request.python_executable_path,
        &request.config_package_name,
    )
    .await?;

    Ok(
        engine_version.as_deref() == Some(request.engine_version.as_str())
            && config_version.as_deref() == Some(request.config_version.as_str()),
    )
}

async fn query_distribution_version(
    python_executable_path: &str,
    package_name: &str,
) -> Result<Option<String>, String> {
    let output = Command::new(python_executable_path)
        .arg("-c")
        .arg(
            "import importlib.metadata, sys\n\
             try:\n\
                 print(importlib.metadata.version(sys.argv[1]))\n\
             except importlib.metadata.PackageNotFoundError:\n\
                 pass",
        )
        .arg(package_name)
        .output()
        .await
        .map_err(|error| format!("Failed to query installed packages with Python: {error}"))?;

    if !output.status.success() {
        return Err(format!(
            "Python package query failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if value.is_empty() {
        Ok(None)
    } else {
        Ok(Some(value))
    }
}

async fn ensure_python_toolchain(request: &PrepareRequest) -> Result<(), String> {
    if tokio::fs::try_exists(&request.python_executable_path)
        .await
        .map_err(|error| format!("Failed to check the Python toolchain: {error}"))?
    {
        return Ok(());
    }

    let python_home_path = request.python_home_path.clone();
    let python_toolchain_path = request.python_toolchain_path.clone();
    tokio::task::spawn_blocking(move || {
        unpack_python_toolchain(&python_toolchain_path, &python_home_path)
    })
    .await
    .map_err(|error| format!("Failed to join Python toolchain extraction: {error}"))?
}

fn unpack_python_toolchain(
    python_toolchain_path: &str,
    python_home_path: &str,
) -> Result<(), String> {
    let target = PathBuf::from(python_home_path);
    let parent = target
        .parent()
        .ok_or_else(|| "Prepared Python home path must have a parent directory.".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("Failed to create '{}': {error}", parent.display()))?;
    if target.exists() {
        std::fs::remove_dir_all(&target)
            .map_err(|error| format!("Failed to reset '{}': {error}", target.display()))?;
    }

    let staging = parent.join(format!(
        ".extract-{}-{}",
        std::process::id(),
        time::OffsetDateTime::now_utc().unix_timestamp_nanos()
    ));
    if staging.exists() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    std::fs::create_dir_all(&staging)
        .map_err(|error| format!("Failed to create '{}': {error}", staging.display()))?;

    let archive_file = std::fs::File::open(python_toolchain_path)
        .map_err(|error| format!("Failed to open '{python_toolchain_path}': {error}"))?;
    let decoder = GzDecoder::new(archive_file);
    let mut archive = Archive::new(decoder);
    archive
        .unpack(&staging)
        .map_err(|error| format!("Failed to unpack '{python_toolchain_path}': {error}"))?;

    let extracted_root = detect_python_root(&staging)?;
    std::fs::rename(&extracted_root, &target).map_err(|error| {
        format!(
            "Failed to move '{}' to '{}': {error}",
            extracted_root.display(),
            target.display()
        )
    })?;
    if staging.exists() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    Ok(())
}

fn detect_python_root(staging: &Path) -> Result<PathBuf, String> {
    if staging.join("bin/python3").is_file() {
        return Ok(staging.to_path_buf());
    }

    let entries = std::fs::read_dir(staging)
        .map_err(|error| format!("Failed to inspect '{}': {error}", staging.display()))?;
    let mut dirs = Vec::new();
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("Failed to inspect extracted toolchain: {error}"))?;
        let path = entry.path();
        if path.is_dir() {
            dirs.push(path);
        }
    }

    if dirs.len() == 1 && dirs[0].join("bin/python3").is_file() {
        return Ok(dirs.remove(0));
    }

    Err("The Python toolchain archive did not unpack into a usable runtime.".to_string())
}

async fn install_python_package(
    python_executable_path: &str,
    package_name: &str,
    expected_version: &str,
    wheel_path: &str,
    install_args: &[&str],
    outgoing_tx: mpsc::UnboundedSender<SessionAgentEvent>,
) -> Result<(), String> {
    if query_distribution_version(python_executable_path, package_name)
        .await?
        .as_deref()
        == Some(expected_version)
    {
        let _ = outgoing_tx.send(SessionAgentEvent::Log {
            phase: Some(RunPhase::InstallPackages),
            level: "info".to_string(),
            message: format!("{package_name} {expected_version} already installed"),
            worker_id: None,
        });
        return Ok(());
    }

    let mut command = Command::new(python_executable_path);
    command
        .arg("-m")
        .arg("pip")
        .arg("install")
        .args(install_args)
        .arg(wheel_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("Failed to start pip for {package_name}: {error}"))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("pip did not expose stdout for {package_name}"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| format!("pip did not expose stderr for {package_name}"))?;
    let stdout_task = tokio::spawn(read_log_stream(
        stdout,
        outgoing_tx.clone(),
        RunPhase::InstallPackages,
        "info",
        None,
    ));
    let stderr_task = tokio::spawn(read_log_stream(
        stderr,
        outgoing_tx.clone(),
        RunPhase::InstallPackages,
        "error",
        None,
    ));
    let status = child
        .wait()
        .await
        .map_err(|error| format!("Failed to wait on pip for {package_name}: {error}"))?;
    let stdout_result = stdout_task
        .await
        .map_err(|error| format!("Failed to join pip stdout task: {error}"))?;
    let stderr_result = stderr_task
        .await
        .map_err(|error| format!("Failed to join pip stderr task: {error}"))?;
    stdout_result?;
    stderr_result?;

    if !status.success() {
        return Err(format!(
            "pip install failed for {package_name} {expected_version} with status {status}."
        ));
    }

    Ok(())
}

fn start_shell_worker(
    worker_id: WorkerId,
    cwd: String,
    rows: u16,
    cols: u16,
    internal_tx: mpsc::UnboundedSender<InternalEvent>,
) -> Result<ShellWorkerHandle, String> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| format!("Failed to allocate a PTY: {error}"))?;
    let mut command = CommandBuilder::new(DEFAULT_SHELL_PATH);
    command.arg("-i");
    command.cwd(cwd);
    let mut child = pair
        .slave
        .spawn_command(command)
        .map_err(|error| format!("Failed to start the interactive shell: {error}"))?;
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| format!("Failed to clone the PTY reader: {error}"))?;
    let mut writer = pair
        .master
        .take_writer()
        .map_err(|error| format!("Failed to acquire the PTY writer: {error}"))?;
    let killer = child.clone_killer();
    let master = pair.master;
    let (control_tx, control_rx) = std_mpsc::channel::<ShellControl>();

    let reader_worker_id = worker_id.clone();
    let reader_internal_tx = internal_tx.clone();
    thread::spawn(move || {
        let mut buffer = [0_u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    let data = String::from_utf8_lossy(&buffer[..read]).into_owned();
                    let _ = reader_internal_tx.send(InternalEvent::Session(
                        SessionAgentEvent::PtyOutput {
                            data,
                            worker_id: reader_worker_id.clone(),
                        },
                    ));
                }
                Err(error) => {
                    let _ =
                        reader_internal_tx.send(InternalEvent::Session(SessionAgentEvent::Error {
                            message: format!("Shell PTY read failed: {error}"),
                            phase: None,
                            retriable: true,
                            worker_id: Some(reader_worker_id.clone()),
                        }));
                    break;
                }
            }
        }
    });

    let wait_worker_id = worker_id.clone();
    let wait_internal_tx = internal_tx.clone();
    thread::spawn(move || match child.wait() {
        Ok(status) => {
            let _ = wait_internal_tx.send(InternalEvent::Session(SessionAgentEvent::WorkerExit {
                code: Some(status.exit_code() as i32),
                kind: WorkerKind::Shell,
                output_path: None,
                validation_issues: None,
                worker_id: wait_worker_id,
            }));
        }
        Err(error) => {
            let _ = wait_internal_tx.send(InternalEvent::Session(SessionAgentEvent::Error {
                message: format!("Shell worker wait failed: {error}"),
                phase: None,
                retriable: true,
                worker_id: Some(wait_worker_id.clone()),
            }));
            let _ = wait_internal_tx.send(InternalEvent::Session(SessionAgentEvent::WorkerExit {
                code: None,
                kind: WorkerKind::Shell,
                output_path: None,
                validation_issues: None,
                worker_id: wait_worker_id,
            }));
        }
    });

    let control_worker_id = worker_id.clone();
    let control_internal_tx = internal_tx;
    thread::spawn(move || {
        let mut killer = killer;
        while let Ok(control) = control_rx.recv() {
            match control {
                ShellControl::Close => {
                    let _ = killer.kill();
                    break;
                }
                ShellControl::Input(data) => {
                    if let Err(error) = writer.write_all(data.as_bytes()) {
                        let _ = control_internal_tx.send(InternalEvent::Session(
                            SessionAgentEvent::Error {
                                message: format!("Failed to write to the PTY: {error}"),
                                phase: None,
                                retriable: true,
                                worker_id: Some(control_worker_id.clone()),
                            },
                        ));
                        let _ = killer.kill();
                        break;
                    }
                    let _ = writer.flush();
                }
                ShellControl::Resize { cols, rows } => {
                    if let Err(error) = master.resize(PtySize {
                        rows,
                        cols,
                        pixel_width: 0,
                        pixel_height: 0,
                    }) {
                        let _ = control_internal_tx.send(InternalEvent::Session(
                            SessionAgentEvent::Error {
                                message: format!("Failed to resize the PTY: {error}"),
                                phase: None,
                                retriable: false,
                                worker_id: Some(control_worker_id.clone()),
                            },
                        ));
                    }
                }
            }
        }
    });

    Ok(ShellWorkerHandle { control_tx })
}

#[derive(Clone)]
struct StartRunRequest {
    input_download: SessionArtifactAccess,
    local_input_path: String,
    local_output_dir: String,
    output_path: String,
    output_upload: SessionArtifactAccess,
    timeout_in_seconds: u64,
    worker_id: WorkerId,
}

struct ArtifactRequestError {
    message: String,
    retriable: bool,
}

fn start_run_worker(
    prepared: PreparedRuntime,
    request: StartRunRequest,
    internal_tx: mpsc::UnboundedSender<InternalEvent>,
) -> RunWorkerHandle {
    let (cancel_tx, cancel_rx) = watch::channel(false);
    tokio::spawn(async move {
        run_worker_task(prepared, request, cancel_rx, internal_tx).await;
    });
    RunWorkerHandle { cancel_tx }
}

async fn run_worker_task(
    prepared: PreparedRuntime,
    request: StartRunRequest,
    mut cancel_rx: watch::Receiver<bool>,
    internal_tx: mpsc::UnboundedSender<InternalEvent>,
) {
    let worker_id = request.worker_id.clone();
    let client = match Client::builder()
        .connect_timeout(ARTIFACT_REQUEST_CONNECT_TIMEOUT)
        .timeout(ARTIFACT_REQUEST_TIMEOUT)
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            emit_worker_error(
                &internal_tx,
                request.worker_id.clone(),
                RunPhase::ExecuteRun,
                format!("Failed to build the artifact HTTP client: {error}"),
                true,
            );
            emit_worker_exit(
                &internal_tx,
                request.worker_id,
                WorkerKind::Run,
                None,
                None,
                None,
            );
            return;
        }
    };

    if *cancel_rx.borrow() {
        let _ = internal_tx.send(InternalEvent::Session(SessionAgentEvent::WorkerExit {
            code: Some(130),
            kind: WorkerKind::Run,
            output_path: None,
            validation_issues: None,
            worker_id,
        }));
        return;
    }

    match download_input(&client, &request.input_download, &request.local_input_path).await {
        Ok(()) => {}
        Err(error) => {
            emit_worker_error(
                &internal_tx,
                request.worker_id.clone(),
                RunPhase::ExecuteRun,
                error.message,
                error.retriable,
            );
            emit_worker_exit(
                &internal_tx,
                request.worker_id,
                WorkerKind::Run,
                None,
                None,
                None,
            );
            return;
        }
    }

    if *cancel_rx.borrow() {
        emit_worker_exit(
            &internal_tx,
            request.worker_id,
            WorkerKind::Run,
            Some(130),
            None,
            None,
        );
        return;
    }

    if let Err(error) = tokio::fs::create_dir_all(&request.local_output_dir).await {
        emit_worker_error(
            &internal_tx,
            request.worker_id.clone(),
            RunPhase::ExecuteRun,
            format!("Failed to create '{}': {error}", request.local_output_dir),
            false,
        );
        emit_worker_exit(
            &internal_tx,
            request.worker_id,
            WorkerKind::Run,
            None,
            None,
            None,
        );
        return;
    }

    let mut command = Command::new(&prepared.python_executable_path);
    command
        .arg("-c")
        .arg(PROCESSOR_CODE)
        .arg(&request.local_input_path)
        .arg(&request.local_output_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            emit_worker_error(
                &internal_tx,
                request.worker_id.clone(),
                RunPhase::ExecuteRun,
                format!("Failed to start the ADE processor: {error}"),
                true,
            );
            emit_worker_exit(
                &internal_tx,
                request.worker_id,
                WorkerKind::Run,
                None,
                None,
                None,
            );
            return;
        }
    };

    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            emit_worker_error(
                &internal_tx,
                request.worker_id.clone(),
                RunPhase::ExecuteRun,
                "ADE processor stdout was not available.".to_string(),
                true,
            );
            let _ = child.kill().await;
            emit_worker_exit(
                &internal_tx,
                request.worker_id,
                WorkerKind::Run,
                None,
                None,
                None,
            );
            return;
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            emit_worker_error(
                &internal_tx,
                request.worker_id.clone(),
                RunPhase::ExecuteRun,
                "ADE processor stderr was not available.".to_string(),
                true,
            );
            let _ = child.kill().await;
            emit_worker_exit(
                &internal_tx,
                request.worker_id,
                WorkerKind::Run,
                None,
                None,
                None,
            );
            return;
        }
    };

    let stdout_task = tokio::spawn(read_run_stdout(
        stdout,
        request.worker_id.clone(),
        internal_tx.clone(),
    ));
    let stderr_task = tokio::spawn(read_run_stderr(
        stderr,
        request.worker_id.clone(),
        internal_tx.clone(),
    ));

    let mut timed_out = false;
    let status = tokio::select! {
        _ = cancel_rx.changed() => {
            let _ = child.kill().await;
            None
        }
        _ = tokio::time::sleep(Duration::from_secs(request.timeout_in_seconds)) => {
            timed_out = true;
            let _ = child.kill().await;
            None
        }
        result = child.wait() => {
            Some(result)
        }
    };

    let stdout_result = stdout_task.await;
    let stderr_result = stderr_task.await;

    let process_result = match stdout_result {
        Ok(Ok(result)) => result,
        Ok(Err(message)) => {
            emit_worker_error(
                &internal_tx,
                request.worker_id.clone(),
                RunPhase::ExecuteRun,
                message,
                true,
            );
            emit_worker_exit(
                &internal_tx,
                request.worker_id,
                WorkerKind::Run,
                None,
                None,
                None,
            );
            return;
        }
        Err(error) => {
            emit_worker_error(
                &internal_tx,
                request.worker_id.clone(),
                RunPhase::ExecuteRun,
                format!("Failed to join the ADE stdout reader: {error}"),
                true,
            );
            emit_worker_exit(
                &internal_tx,
                request.worker_id,
                WorkerKind::Run,
                None,
                None,
                None,
            );
            return;
        }
    };

    match stderr_result {
        Ok(Ok(())) => {}
        Ok(Err(message)) => {
            emit_worker_error(
                &internal_tx,
                request.worker_id.clone(),
                RunPhase::ExecuteRun,
                message,
                true,
            );
            emit_worker_exit(
                &internal_tx,
                request.worker_id,
                WorkerKind::Run,
                None,
                None,
                None,
            );
            return;
        }
        Err(error) => {
            emit_worker_error(
                &internal_tx,
                request.worker_id.clone(),
                RunPhase::ExecuteRun,
                format!("Failed to join the ADE stderr reader: {error}"),
                true,
            );
            emit_worker_exit(
                &internal_tx,
                request.worker_id,
                WorkerKind::Run,
                None,
                None,
                None,
            );
            return;
        }
    }

    if timed_out {
        emit_worker_error(
            &internal_tx,
            request.worker_id.clone(),
            RunPhase::ExecuteRun,
            format!(
                "ADE processor timed out after {} seconds.",
                request.timeout_in_seconds
            ),
            false,
        );
        emit_worker_exit(
            &internal_tx,
            request.worker_id,
            WorkerKind::Run,
            Some(124),
            None,
            None,
        );
        return;
    }

    let Some(status_result) = status else {
        emit_worker_exit(
            &internal_tx,
            request.worker_id,
            WorkerKind::Run,
            Some(130),
            None,
            None,
        );
        return;
    };
    let status = match status_result {
        Ok(status) => status,
        Err(error) => {
            emit_worker_error(
                &internal_tx,
                request.worker_id.clone(),
                RunPhase::ExecuteRun,
                format!("Failed to wait for the ADE processor: {error}"),
                true,
            );
            emit_worker_exit(
                &internal_tx,
                request.worker_id,
                WorkerKind::Run,
                None,
                None,
                None,
            );
            return;
        }
    };

    if !status.success() {
        emit_worker_error(
            &internal_tx,
            request.worker_id.clone(),
            RunPhase::ExecuteRun,
            format!("ADE processor exited with status {status}."),
            false,
        );
        emit_worker_exit(
            &internal_tx,
            request.worker_id,
            WorkerKind::Run,
            status.code(),
            None,
            None,
        );
        return;
    }

    let Some(process_result) = process_result else {
        emit_worker_error(
            &internal_tx,
            request.worker_id.clone(),
            RunPhase::ExecuteRun,
            "ADE processor did not emit a structured result.".to_string(),
            true,
        );
        emit_worker_exit(
            &internal_tx,
            request.worker_id,
            WorkerKind::Run,
            None,
            None,
            None,
        );
        return;
    };

    let upload_body = match tokio::fs::read(&process_result.local_output_path).await {
        Ok(body) => body,
        Err(error) => {
            emit_worker_error(
                &internal_tx,
                request.worker_id.clone(),
                RunPhase::PersistOutputs,
                format!(
                    "Failed to read '{}' for upload: {error}",
                    process_result.local_output_path
                ),
                false,
            );
            emit_worker_exit(
                &internal_tx,
                request.worker_id,
                WorkerKind::Run,
                None,
                None,
                None,
            );
            return;
        }
    };

    if let Err(error) = request_artifact(&client, &request.output_upload, Some(upload_body))
        .await
        .map(|_| ())
    {
        emit_worker_error(
            &internal_tx,
            request.worker_id.clone(),
            RunPhase::PersistOutputs,
            error.message,
            error.retriable,
        );
        emit_worker_exit(
            &internal_tx,
            request.worker_id,
            WorkerKind::Run,
            None,
            None,
            None,
        );
        return;
    }

    emit_worker_exit(
        &internal_tx,
        request.worker_id,
        WorkerKind::Run,
        Some(0),
        Some(request.output_path),
        Some(process_result.validation_issues),
    );
}

async fn download_input(
    client: &Client,
    access: &SessionArtifactAccess,
    local_input_path: &str,
) -> Result<(), ArtifactRequestError> {
    let body = request_artifact(client, access, None).await?;
    let local_input_path = PathBuf::from(local_input_path);
    if let Some(parent) = local_input_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| ArtifactRequestError {
                message: format!("Failed to create '{}': {error}", parent.display()),
                retriable: false,
            })?;
    }
    tokio::fs::write(&local_input_path, body)
        .await
        .map_err(|error| ArtifactRequestError {
            message: format!("Failed to write '{}': {error}", local_input_path.display()),
            retriable: false,
        })
}

async fn request_artifact(
    client: &Client,
    access: &SessionArtifactAccess,
    body: Option<Vec<u8>>,
) -> Result<Vec<u8>, ArtifactRequestError> {
    let method =
        Method::from_bytes(access.method.as_bytes()).map_err(|error| ArtifactRequestError {
            message: format!(
                "Invalid artifact access method '{}': {error}",
                access.method
            ),
            retriable: false,
        })?;
    let mut request = client.request(method, access.url.as_str());
    for (name, value) in &access.headers {
        request = request.header(name, value);
    }
    if let Some(body) = body {
        request = request.body(body);
    }

    let response = request.send().await.map_err(map_artifact_request_error)?;
    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .map_err(|error| ArtifactRequestError {
            message: format!("Failed to read the artifact response body: {error}"),
            retriable: true,
        })?;
    if !status.is_success() {
        let body = String::from_utf8_lossy(&bytes).trim().to_string();
        return Err(ArtifactRequestError {
            message: if body.is_empty() {
                format!("Artifact request returned HTTP {status}.")
            } else {
                format!("Artifact request returned HTTP {status}: {body}")
            },
            retriable: status.is_server_error()
                || status == reqwest::StatusCode::REQUEST_TIMEOUT
                || status == reqwest::StatusCode::TOO_MANY_REQUESTS,
        });
    }
    Ok(bytes.to_vec())
}

fn map_artifact_request_error(error: reqwest::Error) -> ArtifactRequestError {
    if error.is_timeout() {
        return ArtifactRequestError {
            message: "Artifact request timed out.".to_string(),
            retriable: true,
        };
    }

    if error.is_connect() || error.is_request() {
        return ArtifactRequestError {
            message: "Artifact service is unavailable.".to_string(),
            retriable: true,
        };
    }

    ArtifactRequestError {
        message: format!("Artifact request failed: {error}"),
        retriable: true,
    }
}

async fn read_log_stream(
    stream: impl tokio::io::AsyncRead + Unpin,
    outgoing_tx: mpsc::UnboundedSender<SessionAgentEvent>,
    phase: RunPhase,
    level: &'static str,
    worker_id: Option<WorkerId>,
) -> Result<(), String> {
    let mut lines = BufReader::new(stream).lines();
    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|error| format!("Failed to read subprocess logs: {error}"))?
    {
        if line.trim().is_empty() {
            continue;
        }
        let _ = outgoing_tx.send(SessionAgentEvent::Log {
            phase: Some(phase),
            level: level.to_string(),
            message: line,
            worker_id: worker_id.clone(),
        });
    }
    Ok(())
}

async fn read_run_stdout(
    stream: impl tokio::io::AsyncRead + Unpin,
    worker_id: WorkerId,
    internal_tx: mpsc::UnboundedSender<InternalEvent>,
) -> Result<Option<ProcessResult>, String> {
    let mut lines = BufReader::new(stream).lines();
    let mut process_result = None;
    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|error| format!("Failed to read ADE stdout: {error}"))?
    {
        if let Some(payload) = line.strip_prefix(PROCESS_RESULT_PREFIX) {
            process_result = Some(
                serde_json::from_str::<ProcessResult>(payload)
                    .map_err(|error| format!("Invalid ADE process result payload: {error}"))?,
            );
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        let _ = internal_tx.send(InternalEvent::Session(SessionAgentEvent::Stdout {
            data: line,
            phase: Some(RunPhase::ExecuteRun),
            worker_id: worker_id.clone(),
        }));
    }
    Ok(process_result)
}

async fn read_run_stderr(
    stream: impl tokio::io::AsyncRead + Unpin,
    worker_id: WorkerId,
    internal_tx: mpsc::UnboundedSender<InternalEvent>,
) -> Result<(), String> {
    let mut lines = BufReader::new(stream).lines();
    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|error| format!("Failed to read ADE stderr: {error}"))?
    {
        if line.trim().is_empty() {
            continue;
        }
        let _ = internal_tx.send(InternalEvent::Session(SessionAgentEvent::Stderr {
            data: line,
            phase: Some(RunPhase::ExecuteRun),
            worker_id: worker_id.clone(),
        }));
    }
    Ok(())
}

fn emit_worker_error(
    internal_tx: &mpsc::UnboundedSender<InternalEvent>,
    worker_id: WorkerId,
    phase: RunPhase,
    message: String,
    retriable: bool,
) {
    let _ = internal_tx.send(InternalEvent::Session(SessionAgentEvent::Error {
        message,
        phase: Some(phase),
        retriable,
        worker_id: Some(worker_id),
    }));
}

fn emit_worker_exit(
    internal_tx: &mpsc::UnboundedSender<InternalEvent>,
    worker_id: WorkerId,
    kind: WorkerKind,
    code: Option<i32>,
    output_path: Option<String>,
    validation_issues: Option<Vec<RunValidationIssue>>,
) {
    let _ = internal_tx.send(InternalEvent::Session(SessionAgentEvent::WorkerExit {
        code,
        kind,
        output_path,
        validation_issues,
        worker_id,
    }));
}

fn io_error(message: String) -> std::io::Error {
    std::io::Error::other(message)
}

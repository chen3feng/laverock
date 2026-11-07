//! Background jobs with replayable Server-Sent Events.
//!
//! Long operations (synthesis, audio tools, dubbing) used to report
//! progress inside the request that started them: closing the tab or
//! refreshing the page lost the work and there was no way to cancel it.
//! A [`Job`] is the durable handle instead — it has an id, a status, an
//! event log for reconnects, and a cancellation flag the worker polls.
//!
//! Events are plain strings already encoded as JSON, so the SSE layer does
//! no per-event serialization and the replay buffer can be shared as-is.

use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    io,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::broadcast;

/// Events kept for replay after a reconnect. Older ones are dropped; the
/// snapshot sent first still carries the current state.
const REPLAY_EVENTS: usize = 256;
/// Finished jobs and their files are removed after this long.
const JOB_RETENTION: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    #[default]
    Queued,
    Running,
    Done,
    Error,
    Cancelled,
}

impl JobStatus {
    /// Whether no further event will arrive for this job.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            JobStatus::Done | JobStatus::Error | JobStatus::Cancelled
        )
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ArtifactView {
    pub name: String,
    pub bytes: u64,
    pub url: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobSnapshot {
    pub id: String,
    pub kind: String,
    pub status: JobStatus,
    pub progress: f32,
    pub message: String,
    pub created_at: u64,
    pub artifacts: Vec<ArtifactView>,
}

#[derive(Debug, Clone)]
pub struct JobEvent {
    pub kind: String,
    pub data: String,
}

struct Artifact {
    name: String,
    path: PathBuf,
    bytes: u64,
}

#[derive(Default)]
struct JobState {
    status: JobStatus,
    progress: f32,
    message: String,
    log: String,
    artifacts: Vec<Artifact>,
    events: VecDeque<(u64, JobEvent)>,
}

type Killer = Box<dyn Fn() + Send + Sync>;

/// One background operation. Cloning the `Arc` is how the worker, the HTTP
/// handlers and the SSE stream share it.
pub struct Job {
    id: String,
    kind: String,
    created_at: u64,
    dir: PathBuf,
    cancelled: AtomicBool,
    seq: AtomicU64,
    state: Mutex<JobState>,
    tx: broadcast::Sender<(u64, JobEvent)>,
    /// Called by [`Job::cancel`] so a worker owning a child process can stop
    /// it instead of waiting for the current step to finish.
    killer: Mutex<Option<Killer>>,
}

impl Job {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Ask the worker to stop. Idempotent; also stops a registered child.
    pub fn cancel(&self) {
        if self.cancelled.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Some(kill) = self.killer.lock().expect("job killer lock").as_ref() {
            kill();
        }
    }

    /// Register how to stop the work in flight (e.g. kill a child process).
    /// Workers that own a child use this; the framework itself stays
    /// process-agnostic.
    #[allow(dead_code)]
    pub fn on_cancel(&self, killer: impl Fn() + Send + Sync + 'static) {
        *self.killer.lock().expect("job killer lock") = Some(Box::new(killer));
    }

    pub fn start(&self) {
        let mut state = self.state.lock().expect("job state lock");
        state.status = JobStatus::Running;
        state.message = "running".to_string();
    }

    /// Update the numeric progress and, optionally, the human message.
    pub fn set_progress(&self, value: f32, message: Option<&str>) {
        let value = value.clamp(0.0, 1.0);
        {
            let mut state = self.state.lock().expect("job state lock");
            state.progress = value;
            if let Some(message) = message {
                state.message = message.to_string();
            }
        }
        self.emit("progress", json!({ "value": value }));
    }

    /// Append a line to the job log (shown in a details panel / on failure).
    pub fn log(&self, line: &str) {
        let mut state = self.state.lock().expect("job state lock");
        if !state.log.is_empty() {
            state.log.push('\n');
        }
        state.log.push_str(line);
    }

    /// Publish one event to the replay buffer and live subscribers.
    pub fn emit(&self, kind: &str, data: Value) {
        let event = JobEvent {
            kind: kind.to_string(),
            data: data.to_string(),
        };
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        {
            let mut state = self.state.lock().expect("job state lock");
            if state.events.len() == REPLAY_EVENTS {
                state.events.pop_front();
            }
            state.events.push_back((seq, event.clone()));
        }
        let _ = self.tx.send((seq, event));
    }

    /// Mark the job finished successfully and emit `done`.
    pub fn finish(&self, count: u32) {
        {
            let mut state = self.state.lock().expect("job state lock");
            state.status = JobStatus::Done;
            state.progress = 1.0;
            state.message = "done".to_string();
        }
        self.emit("done", json!({ "count": count }));
    }

    /// Mark the job cancelled. The worker keeps whatever it produced.
    pub fn finish_cancelled(&self, count: u32) {
        {
            let mut state = self.state.lock().expect("job state lock");
            state.status = JobStatus::Cancelled;
            state.message = "cancelled".to_string();
        }
        self.emit("done", json!({ "count": count, "cancelled": true }));
    }

    /// Mark the job failed and emit `error`.
    pub fn fail(&self, message: &str) {
        {
            let mut state = self.state.lock().expect("job state lock");
            state.status = JobStatus::Error;
            state.message = message.to_string();
        }
        self.emit("error", json!({ "message": message }));
    }

    /// Write an artifact into the job directory and announce it.
    ///
    /// The name must be a plain file name: the download route looks the
    /// exact name up again, so `..` and separators can never escape.
    pub fn artifact(&self, name: &str, bytes: &[u8]) -> io::Result<ArtifactView> {
        if !safe_name(name) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unsafe artifact name: {name}"),
            ));
        }
        let path = self.dir.join(name);
        std::fs::write(&path, bytes)?;
        let view = ArtifactView {
            name: name.to_string(),
            bytes: bytes.len() as u64,
            url: format!("/api/jobs/{}/files/{}", self.id, name),
        };
        {
            let mut state = self.state.lock().expect("job state lock");
            state.artifacts.retain(|a| a.name != name);
            state.artifacts.push(Artifact {
                name: name.to_string(),
                path,
                bytes: bytes.len() as u64,
            });
        }
        self.emit(
            "artifact",
            serde_json::to_value(&view).unwrap_or(Value::Null),
        );
        Ok(view)
    }

    /// Path of a previously written artifact, if `name` names one exactly.
    pub fn artifact_path(&self, name: &str) -> Option<PathBuf> {
        if !safe_name(name) {
            return None;
        }
        self.state
            .lock()
            .expect("job state lock")
            .artifacts
            .iter()
            .find(|a| a.name == name)
            .map(|a| a.path.clone())
    }

    pub fn snapshot(&self) -> JobSnapshot {
        let state = self.state.lock().expect("job state lock");
        JobSnapshot {
            id: self.id.clone(),
            kind: self.kind.clone(),
            status: state.status,
            progress: state.progress,
            message: state.message.clone(),
            created_at: self.created_at,
            artifacts: state
                .artifacts
                .iter()
                .map(|a| ArtifactView {
                    name: a.name.clone(),
                    bytes: a.bytes,
                    url: format!("/api/jobs/{}/files/{}", self.id, a.name),
                })
                .collect(),
        }
    }

    /// Subscribe to live events. Call before reading the snapshot so no
    /// event can slip between the two.
    pub fn subscribe(&self) -> broadcast::Receiver<(u64, JobEvent)> {
        self.tx.subscribe()
    }

    /// Buffered events, oldest first, for a reconnecting client.
    pub fn replay(&self) -> Vec<(u64, JobEvent)> {
        self.state
            .lock()
            .expect("job state lock")
            .events
            .iter()
            .cloned()
            .collect()
    }

    /// The text log, for a details panel.
    #[allow(dead_code)]
    pub fn log_text(&self) -> String {
        self.state.lock().expect("job state lock").log.clone()
    }
}

/// All jobs of this server process, keyed by id.
pub struct JobStore {
    root: PathBuf,
    jobs: Mutex<HashMap<String, Arc<Job>>>,
    counter: AtomicU64,
}

impl JobStore {
    pub fn open(data_dir: &Path) -> io::Result<Self> {
        let root = data_dir.join("jobs");
        std::fs::create_dir_all(&root)?;
        prune_old(&root);
        Ok(Self {
            root,
            jobs: Mutex::new(HashMap::new()),
            counter: AtomicU64::new(0),
        })
    }

    /// Create a queued job and its artifact directory.
    pub fn create(&self, kind: &str) -> io::Result<Arc<Job>> {
        let id = format!(
            "{:x}-{:x}",
            now_millis(),
            self.counter.fetch_add(1, Ordering::SeqCst)
        );
        let dir = self.root.join(&id);
        std::fs::create_dir_all(&dir)?;
        let (tx, _) = broadcast::channel(REPLAY_EVENTS);
        let job = Arc::new(Job {
            id: id.clone(),
            kind: kind.to_string(),
            created_at: now_secs(),
            dir,
            cancelled: AtomicBool::new(false),
            seq: AtomicU64::new(0),
            state: Mutex::new(JobState::default()),
            tx,
            killer: Mutex::new(None),
        });
        let mut jobs = self.jobs.lock().expect("job store lock");
        self.prune(&mut jobs);
        jobs.insert(id, job.clone());
        Ok(job)
    }

    /// Drop finished jobs past the retention window from the in-memory map.
    /// Their artifact directories are pruned on the next startup.
    fn prune(&self, jobs: &mut HashMap<String, Arc<Job>>) {
        let cutoff = now_secs().saturating_sub(JOB_RETENTION.as_secs());
        jobs.retain(|_, job| job.created_at >= cutoff || !job.snapshot().status.is_terminal());
    }

    pub fn get(&self, id: &str) -> Option<Arc<Job>> {
        self.jobs.lock().expect("job store lock").get(id).cloned()
    }
}

/// Artifact names are single path components made of URL-safe characters.
fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('.')
        && !name.contains("..")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Remove artifact directories left by a previous process once they are
/// older than the retention window.
fn prune_old(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        let old = meta
            .modified()
            .ok()
            .and_then(|m| SystemTime::now().duration_since(m).ok())
            .is_some_and(|age| age > JOB_RETENTION);
        if old {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::process::{Command, Stdio};

    fn store(name: &str) -> (JobStore, PathBuf) {
        let dir = std::env::temp_dir().join(format!("laverock-jobs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        (JobStore::open(&dir).unwrap(), dir)
    }

    #[test]
    fn state_machine_runs_to_done() {
        let (store, dir) = store("state");
        let job = store.create("test").unwrap();
        assert_eq!(job.snapshot().status, JobStatus::Queued);

        job.start();
        assert_eq!(job.snapshot().status, JobStatus::Running);

        job.set_progress(0.4, Some("halfway"));
        let snapshot = job.snapshot();
        assert_eq!(snapshot.progress, 0.4);
        assert_eq!(snapshot.message, "halfway");

        job.finish(2);
        let snapshot = job.snapshot();
        assert_eq!(snapshot.status, JobStatus::Done);
        assert_eq!(snapshot.progress, 1.0);
        let kinds: Vec<String> = job.replay().into_iter().map(|(_, e)| e.kind).collect();
        assert_eq!(kinds, vec!["progress", "done"]);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn failing_and_cancelling_are_terminal() {
        let (store, dir) = store("terminal");
        let job = store.create("test").unwrap();
        job.fail("boom");
        assert_eq!(job.snapshot().status, JobStatus::Error);

        let other = store.create("test").unwrap();
        other.cancel();
        assert!(other.is_cancelled());
        other.finish_cancelled(0);
        assert_eq!(other.snapshot().status, JobStatus::Cancelled);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn replay_is_bounded() {
        let (store, dir) = store("replay");
        let job = store.create("test").unwrap();
        for i in 0..(REPLAY_EVENTS + 10) {
            job.emit("stage", json!({ "message": i }));
        }
        assert_eq!(job.replay().len(), REPLAY_EVENTS);
        let (seq, _) = job.replay().pop().unwrap();
        assert_eq!(seq, (REPLAY_EVENTS + 10) as u64);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn artifacts_reject_traversal_and_serve_known_names() {
        let (store, dir) = store("artifacts");
        let job = store.create("test").unwrap();
        assert!(job.artifact("../escape.wav", b"x").is_err());
        assert!(job.artifact("a/b.wav", b"x").is_err());
        assert!(job.artifact(".hidden", b"x").is_err());

        let view = job.artifact("vocals.wav", b"RIFF").unwrap();
        assert_eq!(view.bytes, 4);
        assert_eq!(view.url, format!("/api/jobs/{}/files/vocals.wav", job.id()));
        assert!(job.artifact_path("vocals.wav").unwrap().is_file());
        assert!(job.artifact_path("../escape.wav").is_none());
        assert!(job.artifact_path("missing.wav").is_none());

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    #[cfg(unix)]
    fn cancel_kills_a_child_process() {
        let (store, dir) = store("cancel");
        let job = store.create("test").unwrap();
        let child = Command::new("sleep")
            .arg("30")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let child = Arc::new(Mutex::new(child));
        let to_kill = child.clone();
        job.on_cancel(move || {
            let _ = to_kill.lock().unwrap().kill();
        });

        job.cancel();
        let status = child.lock().unwrap().wait().unwrap();
        assert!(!status.success(), "cancelled child should be killed");

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn subscribers_receive_live_events() {
        let (store, dir) = store("subscribe");
        let job = store.create("test").unwrap();
        let mut rx = job.subscribe();
        job.emit("stage", json!({ "message": "hello" }));
        let (_, event) = rx.try_recv().unwrap();
        assert_eq!(event.kind, "stage");
        assert!(event.data.contains("hello"));
        let _ = std::fs::remove_dir_all(dir);
    }
}

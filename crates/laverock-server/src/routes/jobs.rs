//! Background job status, event stream, cancellation and artifact download.
//!
//! Jobs are created by the feature that starts long work (currently the
//! opt-in `x-laverock-job: 1` synthesize path); these routes are generic.

use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use std::convert::Infallible;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use super::{ApiError, AppState};
use crate::jobs::Job;

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/api/jobs/{id}", get(status))
        .route("/api/jobs/{id}/events", get(events))
        .route("/api/jobs/{id}/cancel", post(cancel))
        .route("/api/jobs/{id}/files/{name}", get(file))
}

fn lookup(state: &AppState, id: &str) -> Result<std::sync::Arc<Job>, ApiError> {
    state
        .jobs
        .get(id)
        .ok_or_else(|| ApiError::not_found(format!("job not found: {id}")))
}

async fn status(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Response, ApiError> {
    let job = lookup(&state, &id)?;
    Ok(axum::Json(job.snapshot()).into_response())
}

async fn cancel(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Response, ApiError> {
    let job = lookup(&state, &id)?;
    job.cancel();
    Ok((
        StatusCode::ACCEPTED,
        axum::Json(serde_json::json!({ "cancelled": true })),
    )
        .into_response())
}

async fn file(
    Path((id, name)): Path<(String, String)>,
    State(state): State<AppState>,
) -> Result<Response, ApiError> {
    let job = lookup(&state, &id)?;
    // The exact-name lookup is the traversal guard: only files this job
    // announced can be read, and names are single URL-safe components.
    let path = job
        .artifact_path(&name)
        .ok_or_else(|| ApiError::not_found(format!("file not found: {name}")))?;
    let bytes = tokio::fs::read(&path).await?;
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type(&name).to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{name}\""),
            ),
        ],
        bytes,
    )
        .into_response())
}

/// Replay buffered events, then follow live ones until the job ends.
async fn events(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Response, ApiError> {
    Ok(event_stream(lookup(&state, &id)?))
}

/// Build the SSE response for `job`: snapshot first, then the replay
/// buffer, then live events. Split out from the handler so the protocol
/// can be tested without the full server state.
fn event_stream(job: std::sync::Arc<Job>) -> Response {
    // Subscribe before reading the snapshot/replay so no event can slip
    // through the gap; duplicates are filtered by sequence number.
    let mut live = job.subscribe();
    let snapshot = job.snapshot();
    let replay = job.replay();
    let is_terminal = |kind: &str| matches!(kind, "done" | "error");
    // A job can reach its terminal state between the snapshot and the
    // replay read; in that case the terminal event is already buffered.
    let replayed_terminal = replay.iter().any(|(_, event)| is_terminal(&event.kind));

    let (tx, rx) = mpsc::channel::<Result<axum::body::Bytes, Infallible>>(32);
    tokio::spawn(async move {
        let frame = |kind: &str, data: &str| {
            Ok(axum::body::Bytes::from(format!(
                "event: {kind}\ndata: {data}\n\n"
            )))
        };
        let snapshot_json = match serde_json::to_string(&snapshot) {
            Ok(json) => json,
            Err(_) => return,
        };
        if tx.send(frame("snapshot", &snapshot_json)).await.is_err() {
            return;
        }
        let mut last = 0;
        for (seq, event) in replay {
            if tx.send(frame(&event.kind, &event.data)).await.is_err() {
                return;
            }
            last = last.max(seq);
        }
        if snapshot.status.is_terminal() || replayed_terminal {
            return;
        }
        loop {
            match live.recv().await {
                Ok((seq, event)) if seq > last => {
                    last = seq;
                    let terminal = is_terminal(&event.kind);
                    if tx.send(frame(&event.kind, &event.data)).await.is_err() {
                        return;
                    }
                    // Close the stream once the job is over so a finished
                    // job does not hold the connection (or a test) open.
                    if terminal {
                        return;
                    }
                }
                Ok(_) => {}
                // The client fell behind the buffer: resync from the
                // current snapshot rather than silently skipping events.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    let json = match serde_json::to_string(&job.snapshot()) {
                        Ok(json) => json,
                        Err(_) => return,
                    };
                    if tx.send(frame("snapshot", &json)).await.is_err() {
                        return;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
        }
    });

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(ReceiverStream::new(rx)))
        .unwrap_or_else(|e| ApiError::internal(e.to_string()).into_response())
}

fn content_type(name: &str) -> &'static str {
    match name.rsplit('.').next() {
        Some("wav") => "audio/wav",
        Some("mp3") => "audio/mpeg",
        Some("flac") => "audio/flac",
        Some("ogg") => "audio/ogg",
        Some("json") => "application/json",
        Some("srt") => "application/x-subrip",
        Some("vtt") => "text/vtt",
        Some("txt") => "text/plain; charset=utf-8",
        Some("zip") => "application/zip",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::JobStore;
    use std::path::PathBuf;

    fn store(name: &str) -> (JobStore, PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("laverock-job-routes-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        (JobStore::open(&dir).unwrap(), dir)
    }

    async fn body_text(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn finished_job_replays_snapshot_then_events() {
        let (store, dir) = store("replay");
        let job = store.create("test").unwrap();
        job.start();
        job.emit("stage", serde_json::json!({ "message": "go" }));
        job.finish(1);

        let response = event_stream(job);
        assert_eq!(response.status(), StatusCode::OK);
        let text = body_text(response).await;
        let snapshot = text.find("event: snapshot").expect("snapshot frame");
        let stage = text.find("event: stage").expect("stage frame");
        let done = text.find("event: done").expect("done frame");
        assert!(
            snapshot < stage && stage < done,
            "unexpected order:\n{text}"
        );
        assert!(text.contains("\"status\":\"done\""));
        assert!(text.contains("\"count\":1"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn live_events_follow_the_replay() {
        let (store, dir) = store("live");
        let job = store.create("test").unwrap();
        job.start();
        // The stream subscribes before returning, so events emitted after
        // this point are delivered even while the body is being read.
        let response = event_stream(job.clone());
        job.emit("stage", serde_json::json!({ "message": "live" }));
        job.finish(0);

        let text = body_text(response).await;
        assert!(text.contains("event: stage"));
        assert!(text.contains("\"message\":\"live\""));
        assert!(text.contains("event: done"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn content_types_cover_artifacts() {
        assert_eq!(content_type("take-0.wav"), "audio/wav");
        assert_eq!(content_type("lyrics.srt"), "application/x-subrip");
        assert_eq!(content_type("unknown.bin"), "application/octet-stream");
        assert_eq!(content_type("no-extension"), "application/octet-stream");
    }
}

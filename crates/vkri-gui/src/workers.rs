//! Background workers.
//!
//! GStreamer pipelines run on a dedicated worker thread so a long extraction
//! never blocks the GTK main loop. The worker opens its own `ProjectService`
//! against the same on-disk project (the UI service is idle while a run is
//! active), streams progress through a standard mpsc sender and emits one
//! terminal message. A 100 ms glib timeout drains the receiver on the main
//! thread. Cancellation flips the shared token and is observed inside the
//! extraction loop.

use std::path::PathBuf;
use std::sync::mpsc::Sender;

use vkri_core::extract::CancellationToken;

use crate::state::WorkerMessage;

/// Spawn an extraction worker for one segment.
pub fn spawn_extraction(
    project_dir: PathBuf,
    segment_id: String,
    actor: String,
    cancel: CancellationToken,
    sender: Sender<WorkerMessage>,
) {
    std::thread::spawn(move || {
        let worker_id = segment_id.clone();
        let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut service = match vkri_core::ProjectService::open(&project_dir, &actor) {
                Ok(service) => service,
                Err(error) => {
                    let _ = sender.send(WorkerMessage::RunFailed {
                        segment_id: worker_id.clone(),
                        error: error.to_string(),
                    });
                    return;
                }
            };
            let progress_sender = sender.clone();
            let result = service.run_segment(&worker_id, &cancel, move |progress| {
                let _ = progress_sender.send(WorkerMessage::Progress {
                    frames_seen: progress.frames_seen,
                    keyframes_written: progress.keyframes_written,
                    pts_ns: progress.pts_ns,
                });
            });
            match result {
                Ok(outcome) => {
                    let _ = sender.send(WorkerMessage::RunFinished {
                        segment_id: worker_id.clone(),
                        interrupted: outcome.interrupted,
                        message: outcome.record.message,
                    });
                }
                Err(error) => {
                    let _ = sender.send(WorkerMessage::RunFailed {
                        segment_id: worker_id.clone(),
                        error: error.to_string(),
                    });
                }
            }
        }));
        if run.is_err() {
            let _ = sender.send(WorkerMessage::RunFailed {
                segment_id,
                error: "worker thread panicked".to_string(),
            });
        }
    });
}

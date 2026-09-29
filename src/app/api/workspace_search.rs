use std::time::Duration;

use crate::api::schema::{
    Method, Request, ResponseResult, WorkspaceDirectoryEntry, WorkspaceDirectoryPreview,
    WorkspaceSearchCandidate,
};
use crate::app::{workspace_search_provider, App};

use super::responses::{encode_error, encode_success};

impl App {
    pub(crate) fn handle_deferred_workspace_search_request(
        &self,
        request: Request,
        respond_to: std::sync::mpsc::Sender<String>,
    ) {
        tokio::spawn(async move {
            let response = match request.method {
                Method::WorkspaceSearch(params) => {
                    let result = workspace_search_provider::run_zoxide_query(
                        "zoxide",
                        Duration::from_secs(3),
                        || {},
                    )
                    .await;
                    if let Some(error) = result.error {
                        encode_error(request.id, &error.code, error.message)
                    } else {
                        let mut ranked: Vec<_> = result
                            .candidates
                            .into_iter()
                            .filter_map(|candidate| {
                                candidate
                                    .match_rank(&params.query)
                                    .map(|rank| (rank, candidate))
                            })
                            .collect();
                        ranked.sort_by(|(left_rank, left), (right_rank, right)| {
                            left_rank
                                .cmp(right_rank)
                                .then_with(|| right.score.total_cmp(&left.score))
                                .then_with(|| left.canonical_path.cmp(&right.canonical_path))
                        });
                        let candidates = ranked
                            .into_iter()
                            .take(workspace_search_provider::SEARCH_RESULTS_LIMIT)
                            .map(|(_, candidate)| WorkspaceSearchCandidate {
                                shown_path: candidate.abbreviated_path(),
                                canonical_path: candidate
                                    .canonical_path
                                    .to_string_lossy()
                                    .into_owned(),
                                score: candidate.score,
                            })
                            .collect();
                        encode_success(request.id, ResponseResult::WorkspaceSearch { candidates })
                    }
                }
                Method::WorkspaceDirectoryPreview(params) => {
                    let task = workspace_search_provider::filesystem_task(move || {
                        let canonical = std::fs::canonicalize(params.path)?;
                        let preview =
                            workspace_search_provider::read_directory_preview(&canonical)?;
                        Ok::<_, std::io::Error>(WorkspaceDirectoryPreview {
                            canonical_path: canonical.to_string_lossy().into_owned(),
                            entries: preview
                                .entries
                                .into_iter()
                                .map(|entry| WorkspaceDirectoryEntry {
                                    name: entry.name,
                                    is_dir: entry.is_dir,
                                })
                                .collect(),
                            truncated: preview.truncated,
                        })
                    });
                    match tokio::time::timeout(Duration::from_secs(3), task).await {
                        Ok(Ok(Ok(preview))) => encode_success(
                            request.id,
                            ResponseResult::WorkspaceDirectoryPreview { preview },
                        ),
                        Ok(Ok(Err(error))) => {
                            encode_error(request.id, "directory_preview_failed", error.to_string())
                        }
                        Ok(Err(error)) => {
                            encode_error(request.id, "directory_preview_failed", error.to_string())
                        }
                        Err(_) => encode_error(
                            request.id,
                            "directory_preview_timeout",
                            "Directory preview timed out",
                        ),
                    }
                }
                _ => encode_error(
                    request.id,
                    "invalid_request",
                    "Expected a directory request",
                ),
            };
            let _ = respond_to.send(response);
        });
    }
}

//! Search provider data, process execution, and directory preview I/O for
//! the Workspace Switcher.
//!
//! This module provides:
//! - Pure data types (`SearchProviderCandidate`, `ZoxideDirectory`,
//!   `DirectoryPreview`, lifecycle enums) consumed by the UI layer.
//! - Matching helpers on `SearchProviderCandidate` so the UI never
//!   re-implements basename extraction or ranking logic.
//! - `run_zoxide_query` — spawns `zoxide query --list --score` with a
//!   timeout, draining stdout concurrently to avoid pipe deadlock.
//! - `read_directory_preview` — reads one level of a directory for the
//!   preview pane.
//!
//! The App/HeadlessServer controller layers own *when* to call these
//! functions; the UI layer owns only pure projection/render.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
#[cfg(test)]
use std::time::Instant;

// ---------------------------------------------------------------------------
// Candidate data and matching helpers
// ---------------------------------------------------------------------------

/// One directory suggested by a search provider.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SearchProviderCandidate {
    /// Path as reported by the provider; this is what the row shows.
    pub(crate) shown_path: PathBuf,
    /// Canonical identity used to match open workspaces and dedup aliases.
    pub(crate) canonical_path: PathBuf,
    /// Provider relevance score; higher is more relevant.
    pub(crate) score: f64,
}

impl SearchProviderCandidate {
    /// Build a candidate from a provider-reported path, resolving its
    /// canonical identity with the shared canonical-path helper. The helper
    /// is best-effort: a path that no longer exists keeps its reported path
    /// as identity.
    pub(crate) fn from_shown_path(shown_path: PathBuf, score: f64) -> Self {
        let canonical_path = crate::worktree::canonical_or_original(&shown_path);
        Self {
            shown_path,
            canonical_path,
            score,
        }
    }

    /// Basename of the shown path, or the full display string if there is
    /// no file name component.
    pub(crate) fn basename(&self) -> String {
        self.shown_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.shown_path.display().to_string())
    }

    /// Full display string of the shown path.
    pub(crate) fn display_path(&self) -> String {
        self.shown_path.display().to_string()
    }

    /// Abbreviated display path for row metadata. The home directory is
    /// replaced with `~` on Unix or `%USERPROFILE%` on Windows.
    /// Abbreviation is display-only; the shown_path is preserved for create.
    pub(crate) fn abbreviated_path(&self) -> String {
        abbreviate_home(&self.shown_path)
    }

    /// Compute the match rank for a query against this candidate.
    ///
    /// Returns `Some((quality, position_sum))` when the query matches.
    /// Basename match quality always wins over full-path match quality:
    /// if the basename matches, the basename rank is returned even when a
    /// full-path match would have a numerically better rank.
    pub(crate) fn match_rank(&self, query: &str) -> Option<(u8, usize)> {
        let basename = self.basename();
        if let Some(basename_rank) = crate::workspace_search::match_rank(query, &basename) {
            return Some(basename_rank);
        }
        let path_str = self.display_path();
        crate::workspace_search::match_rank(query, &path_str)
    }
}

/// Abbreviate a path by replacing the home directory prefix with `~` (Unix)
/// or `%USERPROFILE%` (Windows). Falls back to the full display string when
/// the home directory cannot be determined or the path is not under it.
pub(crate) fn abbreviate_home(path: &Path) -> String {
    if let Some(home) = home_dir() {
        if let Ok(rest) = path.strip_prefix(&home) {
            let tilde = if cfg!(windows) { "%USERPROFILE%" } else { "~" };
            return if rest.as_os_str().is_empty() {
                tilde.to_owned()
            } else {
                format!("{tilde}{}{}", std::path::MAIN_SEPARATOR, rest.display())
            };
        }
    }
    path.display().to_string()
}

/// Best-effort home directory resolution. Uses the `HOME` environment
/// variable on Unix and `USERPROFILE` (falling back to `HOMEDRIVE` +
/// `HOMEPATH`) on Windows. Split into cfg-gated bodies to avoid using
/// `cfg!(windows)` for control flow (AGENTS.md policy).
#[cfg(not(windows))]
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[cfg(windows)]
fn home_dir() -> Option<PathBuf> {
    if let Some(profile) = std::env::var_os("USERPROFILE") {
        return Some(PathBuf::from(profile));
    }
    if let (Ok(drive), Ok(path)) = (std::env::var("HOMEDRIVE"), std::env::var("HOMEPATH")) {
        return Some(PathBuf::from(format!("{drive}{path}")));
    }
    None
}

// ---------------------------------------------------------------------------
// zoxide output parsing
// ---------------------------------------------------------------------------

/// One parsed line of `zoxide query --list --score` output.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ZoxideDirectory {
    pub(crate) shown_path: PathBuf,
    pub(crate) score: f64,
}

/// Parse `zoxide query --list --score` output.
///
/// Each line is `<score><whitespace><path>`, with the score column padded
/// for alignment. The path keeps everything after the first whitespace run,
/// so paths containing spaces survive. Malformed lines are skipped.
pub(crate) fn parse_zoxide_list(output: &str) -> Vec<ZoxideDirectory> {
    output
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let (score, path) = line.split_once(char::is_whitespace)?;
            let score: f64 = score.parse().ok()?;
            if !score.is_finite() {
                return None;
            }
            let path = path.trim_start();
            if path.is_empty() {
                return None;
            }
            Some(ZoxideDirectory {
                shown_path: PathBuf::from(path),
                score,
            })
        })
        .collect()
}

/// Collapse aliases that share one canonical identity, keeping the
/// highest-score alias for each identity. Order follows first appearance
/// of each identity.
pub(crate) fn dedup_by_canonical_identity(
    candidates: Vec<SearchProviderCandidate>,
) -> Vec<SearchProviderCandidate> {
    let mut identities: Vec<PathBuf> = Vec::new();
    let mut best: HashMap<PathBuf, SearchProviderCandidate> = HashMap::new();
    for candidate in candidates {
        match best.get_mut(&candidate.canonical_path) {
            Some(existing) => {
                if candidate.score > existing.score {
                    *existing = candidate;
                }
            }
            None => {
                identities.push(candidate.canonical_path.clone());
                best.insert(candidate.canonical_path.clone(), candidate);
            }
        }
    }
    identities
        .into_iter()
        .filter_map(|identity| best.remove(&identity))
        .collect()
}

// ---------------------------------------------------------------------------
// Directory preview
// ---------------------------------------------------------------------------

/// Maximum number of unified result rows the search list shows.
pub(crate) const SEARCH_RESULTS_LIMIT: usize = 100;

/// Maximum number of entries a directory preview lists.
pub(crate) const DIRECTORY_PREVIEW_LIMIT: usize = 200;

/// One entry in a directory preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DirectoryPreviewEntry {
    pub(crate) name: String,
    pub(crate) is_dir: bool,
}

/// One-level snapshot of a directory for the preview pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DirectoryPreview {
    pub(crate) entries: Vec<DirectoryPreviewEntry>,
    /// True when the directory holds more entries than
    /// [`DIRECTORY_PREVIEW_LIMIT`] and the listing was cut short.
    pub(crate) truncated: bool,
}

/// Read one level of `path` for the preview pane.
///
/// Dotfiles are included. Directories sort before files, each group by name.
/// At most [`DIRECTORY_PREVIEW_LIMIT`] entries are returned; `truncated`
/// records whether more entries exist.
pub(crate) fn read_directory_preview(path: &Path) -> io::Result<DirectoryPreview> {
    let mut entries = std::collections::BTreeSet::new();
    let mut truncated = false;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_dir = entry.file_type()?.is_dir();
        entries.insert((!is_dir, name));
        if entries.len() > DIRECTORY_PREVIEW_LIMIT {
            entries.pop_last();
            truncated = true;
        }
    }
    let entries = entries
        .into_iter()
        .map(|(file, name)| DirectoryPreviewEntry {
            name,
            is_dir: !file,
        })
        .collect();
    Ok(DirectoryPreview { entries, truncated })
}

static FILESYSTEM_TASKS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

/// Keep timed-out filesystem calls from accumulating unbounded blocking workers.
pub(crate) async fn filesystem_task<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> io::Result<T> {
    let permit = FILESYSTEM_TASKS.acquire().await.map_err(io::Error::other)?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        work()
    })
    .await
    .map_err(io::Error::other)
}

async fn read_bounded(
    reader: impl tokio::io::AsyncRead + Unpin,
    limit: u64,
) -> io::Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;
    let mut output = Vec::new();
    reader.take(limit + 1).read_to_end(&mut output).await?;
    if output.len() as u64 > limit {
        return Err(io::Error::other("zoxide output limit exceeded"));
    }
    Ok(output)
}

// ---------------------------------------------------------------------------
// Provider lifecycle: availability, query, and result shaping
// ---------------------------------------------------------------------------

/// Outcome of a background zoxide query, reported back to the main loop.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ZoxideQueryResult {
    /// Whether the zoxide binary was found and spawned successfully.
    pub(crate) available: bool,
    /// Parsed and deduplicated candidates. Empty when unavailable or failed.
    pub(crate) candidates: Vec<SearchProviderCandidate>,
    pub(crate) error: Option<crate::api::schema::ErrorBody>,
}

/// Execute the provider without blocking the endpoint's input or render loop.
pub(crate) async fn run_zoxide_query<F>(
    program: &str,
    timeout: Duration,
    on_spawned: F,
) -> ZoxideQueryResult
where
    F: FnOnce(),
{
    let deadline = tokio::time::Instant::now() + timeout;
    let mut command = crate::noninteractive_process::command(program);
    command
        .args(["query", "--list", "--score"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
    let mut command = tokio::process::Command::from(command);
    command.kill_on_drop(true);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return zoxide_failure("workspace_search_unavailable", error.to_string()),
    };
    on_spawned();
    let capture = async {
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("zoxide stdout is unavailable"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| io::Error::other("zoxide stderr is unavailable"))?;
        tokio::try_join!(
            child.wait(),
            read_bounded(stdout, 4 * 1024 * 1024),
            read_bounded(stderr, 64 * 1024)
        )
    };
    let (status, stdout, stderr) = match tokio::time::timeout_at(deadline, capture).await {
        Err(_) => return zoxide_failure("workspace_search_timeout", "Directory search timed out"),
        Ok(Err(error)) => return zoxide_failure("workspace_search_failed", error.to_string()),
        Ok(Ok(output)) => output,
    };
    if !status.success() {
        let detail: String = String::from_utf8_lossy(&stderr)
            .trim()
            .chars()
            .take(512)
            .collect();
        return zoxide_failure(
            "workspace_search_failed",
            format!("zoxide exited with {status}: {detail}"),
        );
    }
    let candidates = filesystem_task(move || {
        let candidates = parse_zoxide_list(&String::from_utf8_lossy(&stdout))
            .into_iter()
            .map(|dir| SearchProviderCandidate::from_shown_path(dir.shown_path, dir.score))
            .collect();
        dedup_by_canonical_identity(candidates)
    });
    match tokio::time::timeout_at(deadline, candidates).await {
        Ok(Ok(candidates)) => ZoxideQueryResult {
            available: true,
            candidates,
            error: None,
        },
        Ok(Err(error)) => zoxide_failure("workspace_search_failed", error.to_string()),
        Err(_) => zoxide_failure("workspace_search_timeout", "Directory search timed out"),
    }
}

fn zoxide_failure(code: &str, message: impl Into<String>) -> ZoxideQueryResult {
    ZoxideQueryResult {
        available: code != "workspace_search_unavailable",
        candidates: Vec::new(),
        error: Some(crate::api::schema::ErrorBody {
            code: code.into(),
            message: message.into(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_TEST_DIR: AtomicUsize = AtomicUsize::new(0);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(name: &str) -> Self {
            let unique = NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "herdr-workspace-search-provider-{name}-{}-{unique}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("create test dir");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn candidate(shown: &str, canonical: &str, score: f64) -> SearchProviderCandidate {
        SearchProviderCandidate {
            shown_path: PathBuf::from(shown),
            canonical_path: PathBuf::from(canonical),
            score,
        }
    }

    #[test]
    fn parse_preserves_paths_with_spaces() {
        let output = "5221.6 /Users/fronz/Developer/plan-app\n\
                      62.8 /Users/fronz/my project/dir with spaces\n";
        let parsed = parse_zoxide_list(output);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].score, 5221.6);
        assert_eq!(
            parsed[0].shown_path,
            PathBuf::from("/Users/fronz/Developer/plan-app")
        );
        assert_eq!(parsed[1].score, 62.8);
        assert_eq!(
            parsed[1].shown_path,
            PathBuf::from("/Users/fronz/my project/dir with spaces")
        );
    }

    #[test]
    fn parse_skips_malformed_lines() {
        let output = "not-a-score /some/path\n\
                      no-whitespace-separator\n\
                      NaN /nan/path\n\
                      inf /infinite/path\n\
                      12.5\n\
                      12.5    \n\
                      \n\
                      7.5 /valid/path\n";
        let parsed = parse_zoxide_list(output);
        assert_eq!(
            parsed,
            vec![ZoxideDirectory {
                shown_path: PathBuf::from("/valid/path"),
                score: 7.5,
            }]
        );
    }

    #[test]
    fn candidate_resolves_canonical_identity() {
        let dir = TestDir::new("canonical");
        let candidate = SearchProviderCandidate::from_shown_path(dir.path().to_path_buf(), 10.0);
        assert_eq!(candidate.shown_path, dir.path());
        assert_eq!(
            candidate.canonical_path,
            fs::canonicalize(dir.path()).expect("canonicalize")
        );
        assert_eq!(candidate.score, 10.0);
    }

    #[test]
    fn candidate_keeps_stale_path_as_identity() {
        let missing = std::env::temp_dir().join(format!(
            "herdr-workspace-search-provider-stale-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&missing);
        let candidate = SearchProviderCandidate::from_shown_path(missing.clone(), 3.0);
        assert_eq!(candidate.shown_path, missing);
        assert_eq!(candidate.canonical_path, missing);
    }

    #[test]
    fn dedup_keeps_highest_score_alias() {
        let candidates = vec![
            candidate("/alias/low", "/identity/one", 10.0),
            candidate("/alias/other", "/identity/two", 5.0),
            candidate("/alias/high", "/identity/one", 20.0),
        ];
        let deduped = dedup_by_canonical_identity(candidates);
        assert_eq!(deduped.len(), 2);
        // Highest-score alias wins even when it arrives later.
        assert_eq!(deduped[0].shown_path, PathBuf::from("/alias/high"));
        assert_eq!(deduped[0].score, 20.0);
        // Identity order follows first appearance.
        assert_eq!(deduped[1].shown_path, PathBuf::from("/alias/other"));
    }

    #[test]
    fn preview_includes_dotfiles_and_sorts_dirs_first() {
        let dir = TestDir::new("ordering");
        fs::create_dir(dir.path().join("zdir")).expect("mkdir zdir");
        fs::create_dir(dir.path().join(".hiddendir")).expect("mkdir .hiddendir");
        fs::write(dir.path().join("bfile"), "").expect("write bfile");
        fs::write(dir.path().join(".afile"), "").expect("write .afile");

        let preview = read_directory_preview(dir.path()).expect("preview");
        assert!(!preview.truncated);
        let names: Vec<(&str, bool)> = preview
            .entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry.is_dir))
            .collect();
        assert_eq!(
            names,
            vec![
                (".hiddendir", true),
                ("zdir", true),
                (".afile", false),
                ("bfile", false),
            ]
        );
    }

    #[test]
    fn preview_limits_entries_and_records_truncation() {
        let dir = TestDir::new("truncation");
        for index in 0..205 {
            fs::write(dir.path().join(format!("file-{index:03}")), "").expect("write file");
        }
        let preview = read_directory_preview(dir.path()).expect("preview");
        assert!(preview.truncated);
        assert_eq!(preview.entries.len(), DIRECTORY_PREVIEW_LIMIT);
        assert_eq!(preview.entries[0].name, "file-000");
        assert_eq!(preview.entries[199].name, "file-199");

        let small = TestDir::new("no-truncation");
        fs::write(small.path().join("only"), "").expect("write file");
        let preview = read_directory_preview(small.path()).expect("preview");
        assert!(!preview.truncated);
        assert_eq!(preview.entries.len(), 1);
    }

    #[test]
    fn preview_errors_for_missing_directory() {
        let missing = std::env::temp_dir().join(format!(
            "herdr-workspace-search-provider-missing-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&missing);
        let result = read_directory_preview(&missing);
        assert!(result.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn preview_errors_for_unreadable_directory() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TestDir::new("unreadable");
        let locked = dir.path().join("locked");
        fs::create_dir(&locked).expect("mkdir locked");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).expect("chmod 000");
        let result = read_directory_preview(&locked);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).expect("restore chmod");
        assert!(result.is_err());
    }

    #[test]
    fn basename_match_wins_over_better_path_rank() {
        // "foo" matches basename of "/a/b/foo" with quality 0 (exact).
        // "foo" also matches path "/a/b/foo" but with a worse (higher) rank
        // because it's not at the start. Basename should win.
        let c = candidate("/a/b/foo", "/a/b/foo", 10.0);
        let rank = c.match_rank("foo").expect("should match");
        // quality 0 = exact match on basename
        assert_eq!(rank.0, 0);
    }

    #[test]
    fn path_match_used_when_basename_does_not_match() {
        // "a/b" matches the path but not the basename "foo".
        let c = candidate("/a/b/foo", "/a/b/foo", 10.0);
        let rank = c.match_rank("a/b").expect("should match via path");
        // Should be a fuzzy/partial match, not exact.
        assert!(rank.0 > 0);
    }

    #[test]
    fn no_match_returns_none() {
        let c = candidate("/a/b/foo", "/a/b/foo", 10.0);
        assert!(c.match_rank("xyz").is_none());
    }

    #[test]
    fn abbreviated_path_replaces_home_with_tilde() {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        if let Some(home) = home {
            let path = home.join("projects/myapp");
            let abbrev = abbreviate_home(&path);
            assert!(abbrev.starts_with('~'));
            assert!(abbrev.ends_with("projects/myapp"));
        }
    }

    #[test]
    fn abbreviated_path_keeps_non_home_paths() {
        let abbrev = abbreviate_home(Path::new("/tmp/some/dir"));
        assert_eq!(abbrev, "/tmp/some/dir");
    }

    // -----------------------------------------------------------------------
    // Provider process tests (controlled executable, no developer database)
    // -----------------------------------------------------------------------

    #[cfg(unix)]
    fn write_fake_zoxide(name: &str, script: &str) -> PathBuf {
        let unique = NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "herdr-fake-zoxide-{name}-{}-{unique}",
            std::process::id()
        ));
        fs::write(&path, format!("#!/bin/sh\n{script}")).expect("write fake zoxide");
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod fake zoxide");
        path
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fork_merge_zoxide_rejects_unbounded_process_output() {
        for stream in ["stdout", "stderr"] {
            let output = if stream == "stdout" { "" } else { " >&2" };
            let fake = write_fake_zoxide(stream, &format!("head -c 5000000 /dev/zero{output}"));
            let result =
                run_zoxide_query(fake.to_str().unwrap(), Duration::from_secs(5), || {}).await;
            assert_eq!(
                result.error.as_ref().map(|error| error.code.as_str()),
                Some("workspace_search_failed")
            );
            assert!(result.error.unwrap().message.contains("output limit"));
            assert!(result.candidates.is_empty());
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fork_merge_zoxide_deadline_covers_all_query_stages() {
        let fake = write_fake_zoxide("deadline", "printf '1 /tmp\n'");
        let result = run_zoxide_query(fake.to_str().unwrap(), Duration::from_millis(5), || {
            std::thread::sleep(Duration::from_millis(20));
        })
        .await;
        assert_eq!(
            result.error.as_ref().map(|error| error.code.as_str()),
            Some("workspace_search_timeout")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_zoxide_query_parses_valid_output() {
        let dir = TestDir::new("zoxide-valid");
        let real = dir.path().join("real");
        fs::create_dir(&real).expect("mkdir real");
        let canonical_real = fs::canonicalize(&real).expect("canonicalize");

        let script = format!(
            "printf '100.0 {}\n50.0 /nonexistent/stale\n'",
            real.display()
        );
        let fake = write_fake_zoxide("valid", &script);

        let result = run_zoxide_query(fake.to_str().unwrap(), Duration::from_secs(5), || {}).await;
        assert!(result.available);
        assert_eq!(result.candidates.len(), 2);
        // The existing directory is canonicalized; the stale one keeps its path.
        assert_eq!(result.candidates[0].canonical_path, canonical_real);
        assert_eq!(result.candidates[0].score, 100.0);
        assert_eq!(
            result.candidates[1].shown_path,
            PathBuf::from("/nonexistent/stale")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_zoxide_query_reports_unavailable_for_missing_binary() {
        let missing =
            std::env::temp_dir().join(format!("herdr-zoxide-missing-{}", std::process::id()));
        let _ = fs::remove_file(&missing);
        let result =
            run_zoxide_query(missing.to_str().unwrap(), Duration::from_secs(5), || {}).await;
        assert!(!result.available);
        assert_eq!(
            result.error.as_ref().map(|error| error.code.as_str()),
            Some("workspace_search_unavailable")
        );
        assert!(result.candidates.is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_zoxide_query_reports_unavailable_for_non_executable() {
        // A file that exists but is not executable should fail to spawn.
        let dir = TestDir::new("non-exec");
        let fake = dir.path().join("not-zoxide");
        fs::write(&fake, "#!/bin/sh\necho should-not-run\n").expect("write fake");
        // Leave permissions as 0o644 (not executable).
        let result = run_zoxide_query(fake.to_str().unwrap(), Duration::from_secs(5), || {}).await;
        assert!(!result.available);
        assert_eq!(
            result.error.as_ref().map(|error| error.code.as_str()),
            Some("workspace_search_unavailable")
        );
        assert!(result.candidates.is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_zoxide_query_times_out() {
        // A script that sleeps longer than the timeout.
        let fake = write_fake_zoxide("slow", "sleep 10");
        let start = Instant::now();
        let result =
            run_zoxide_query(fake.to_str().unwrap(), Duration::from_millis(200), || {}).await;
        let elapsed = start.elapsed();
        assert!(result.available);
        assert!(result.candidates.is_empty());
        assert_eq!(
            result.error.as_ref().map(|error| error.code.as_str()),
            Some("workspace_search_timeout")
        );
        // Should return well before the 10-second sleep finishes.
        assert!(elapsed < Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_zoxide_query_reports_nonzero_exit() {
        let fake = write_fake_zoxide("fail", "echo '100.0 /should-not-appear' >&2; exit 1");
        let result = run_zoxide_query(fake.to_str().unwrap(), Duration::from_secs(5), || {}).await;
        assert_eq!(
            result.error.as_ref().map(|error| error.code.as_str()),
            Some("workspace_search_failed")
        );
        assert!(result.available);
        assert!(result.candidates.is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_zoxide_query_returns_empty_on_malformed_output() {
        let fake = write_fake_zoxide("malformed", "printf 'garbage no score\nbroken\n'");
        let result = run_zoxide_query(fake.to_str().unwrap(), Duration::from_secs(5), || {}).await;
        assert!(result.available);
        assert!(result.candidates.is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_zoxide_query_deduplicates_canonical_aliases() {
        let dir = TestDir::new("zoxide-dedup");
        let real = dir.path().join("real");
        let alias = dir.path().join("alias");
        fs::create_dir(&real).expect("mkdir real");
        {
            use std::os::unix::fs::symlink;
            let _ = symlink(&real, &alias);
        }
        let canonical = fs::canonicalize(&real).expect("canonicalize");

        let script = format!(
            "printf '10.0 {}\n200.0 {}\n'",
            alias.display(),
            real.display()
        );
        let fake = write_fake_zoxide("dedup", &script);

        let result = run_zoxide_query(fake.to_str().unwrap(), Duration::from_secs(5), || {}).await;
        assert!(result.available);
        assert_eq!(result.candidates.len(), 1);
        // Highest-score alias wins.
        assert_eq!(result.candidates[0].canonical_path, canonical);
        assert_eq!(result.candidates[0].score, 200.0);
        assert_eq!(result.candidates[0].shown_path, real);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_zoxide_query_handles_large_output_without_deadlock() {
        // Emit more data than a typical pipe buffer (64 KiB on Linux).
        // Each line is ~30 bytes; 10000 lines ≈ 300 KiB.
        let dir = TestDir::new("zoxide-large");
        let real = dir.path().join("real");
        fs::create_dir(&real).expect("mkdir real");
        let canonical = fs::canonicalize(&real).expect("canonicalize");

        let script = format!(
            "i=0; while [ $i -lt 10000 ]; do printf '%07.1f {}\n' \"$i\"; i=$((i + 1)); done",
            real.display()
        );
        let fake = write_fake_zoxide("large", &script);

        let start = Instant::now();
        let result = run_zoxide_query(fake.to_str().unwrap(), Duration::from_secs(10), || {}).await;
        let elapsed = start.elapsed();

        assert!(result.available);
        // All 10000 lines resolve to the same canonical path, so after
        // dedup there is exactly one candidate.
        assert_eq!(result.candidates.len(), 1);
        assert_eq!(result.candidates[0].canonical_path, canonical);
        // Must finish well before the timeout — no deadlock.
        assert!(elapsed < Duration::from_secs(5));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_zoxide_query_calls_on_spawned_after_successful_spawn() {
        let dir = TestDir::new("zoxide-callback");
        let real = dir.path().join("real");
        fs::create_dir(&real).expect("mkdir real");
        let script = format!("printf '100.0 {}\n'", real.display());
        let fake = write_fake_zoxide("callback", &script);

        let spawned = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let spawned_clone = spawned.clone();
        let result = run_zoxide_query(fake.to_str().unwrap(), Duration::from_secs(5), || {
            spawned_clone.store(true, std::sync::atomic::Ordering::SeqCst);
        })
        .await;
        assert!(result.available);
        assert!(spawned.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_zoxide_query_does_not_call_on_spawned_for_missing_binary() {
        let missing =
            std::env::temp_dir().join(format!("herdr-zoxide-missing-cb-{}", std::process::id()));
        let _ = fs::remove_file(&missing);
        let spawned = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let spawned_clone = spawned.clone();
        let result = run_zoxide_query(missing.to_str().unwrap(), Duration::from_secs(5), || {
            spawned_clone.store(true, std::sync::atomic::Ordering::SeqCst);
        })
        .await;
        assert!(!result.available);
        assert_eq!(
            result.error.as_ref().map(|error| error.code.as_str()),
            Some("workspace_search_unavailable")
        );
        assert!(!spawned.load(std::sync::atomic::Ordering::SeqCst));
    }
}

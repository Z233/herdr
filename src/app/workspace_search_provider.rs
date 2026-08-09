//! Pure data for Workspace Switcher search providers.
//!
//! A search provider turns external directory knowledge (for example the
//! zoxide database) into candidates that the Workspace Switcher merges with
//! open workspaces. This module only parses and shapes data; spawning
//! provider processes and rendering rows happen in the app and ui layers.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

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
}

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
    let mut entries = Vec::new();
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_dir = entry.file_type()?.is_dir();
        entries.push(DirectoryPreviewEntry { name, is_dir });
    }
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name)));
    let truncated = entries.len() > DIRECTORY_PREVIEW_LIMIT;
    entries.truncate(DIRECTORY_PREVIEW_LIMIT);
    Ok(DirectoryPreview { entries, truncated })
}

// ---------------------------------------------------------------------------
// Provider lifecycle: availability, query, and result shaping
// ---------------------------------------------------------------------------

/// Lifecycle status of the search provider for a Search session.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum SearchProviderStatus {
    /// No provider activity yet (Search not entered or not applicable).
    #[default]
    Idle,
    /// A provider query is in flight.
    Loading,
    /// The provider query completed (results may be empty).
    Ready,
    /// No provider binary is available on PATH.
    Unavailable,
}

/// Cached state of a directory preview for one shown path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DirectoryPreviewState {
    /// A preview load is in flight for this path.
    Loading,
    /// The preview was loaded successfully.
    Ready(DirectoryPreview),
    /// The preview load failed (directory missing or unreadable).
    Error,
}

/// Outcome of a background zoxide query, reported back to the main loop.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ZoxideQueryResult {
    /// Whether the zoxide binary was found and the query executed.
    pub(crate) available: bool,
    /// Parsed and deduplicated candidates. Empty when unavailable or failed.
    pub(crate) candidates: Vec<SearchProviderCandidate>,
}

/// Check whether a `zoxide` binary is reachable on PATH.
///
/// This is a best-effort filesystem check that does not spawn a process.
/// It looks for an executable file named `zoxide` (or `zoxide.exe` on
/// Windows) in any PATH directory.
pub(crate) fn zoxide_available() -> bool {
    let Some(path_var) = std::env::var_os("PATH") else {
        return false;
    };
    let binary = if cfg!(windows) {
        "zoxide.exe"
    } else {
        "zoxide"
    };
    std::env::split_paths(&path_var).any(|dir| dir.join(binary).is_file())
}

/// Run `zoxide query --list --score` with a timeout and return parsed
/// candidates.
///
/// The `program` parameter is the binary to invoke (normally `"zoxide"`).
/// Tests pass a controlled executable path so they never touch the
/// developer's zoxide database.
///
/// Returns `available: false` when the binary cannot be spawned. Returns
/// `available: true` with an empty candidate list on timeout, non-zero
/// exit, or malformed output — contributing no rows and no error.
pub(crate) fn run_zoxide_query(program: &str, timeout: Duration) -> ZoxideQueryResult {
    let mut child = match crate::noninteractive_process::command(program)
        .args(["query", "--list", "--score"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => {
            return ZoxideQueryResult {
                available: false,
                candidates: Vec::new(),
            }
        }
    };

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return ZoxideQueryResult {
                        available: true,
                        candidates: Vec::new(),
                    };
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => {
                let _ = child.kill();
                return ZoxideQueryResult {
                    available: true,
                    candidates: Vec::new(),
                };
            }
        }
    };

    if !status.success() {
        return ZoxideQueryResult {
            available: true,
            candidates: Vec::new(),
        };
    }

    let mut stdout = String::new();
    if let Some(mut stdout_handle) = child.stdout.take() {
        use std::io::Read;
        let _ = stdout_handle.read_to_string(&mut stdout);
    }

    let candidates: Vec<_> = parse_zoxide_list(&stdout)
        .into_iter()
        .map(|dir| SearchProviderCandidate::from_shown_path(dir.shown_path, dir.score))
        .collect();
    let candidates = dedup_by_canonical_identity(candidates);
    ZoxideQueryResult {
        available: true,
        candidates,
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
    #[test]
    fn run_zoxide_query_parses_valid_output() {
        let dir = TestDir::new("zoxide-valid");
        let real = dir.path().join("real");
        fs::create_dir(&real).expect("mkdir real");
        let canonical_real = fs::canonicalize(&real).expect("canonicalize");

        let script = format!(
            "printf '100.0 {}\n50.0 /nonexistent/stale\n'",
            real.display()
        );
        let fake = write_fake_zoxide("valid", &script);

        let result = run_zoxide_query(fake.to_str().unwrap(), Duration::from_secs(5));
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
    #[test]
    fn run_zoxide_query_reports_unavailable_for_missing_binary() {
        let missing =
            std::env::temp_dir().join(format!("herdr-zoxide-missing-{}", std::process::id()));
        let _ = fs::remove_file(&missing);
        let result = run_zoxide_query(missing.to_str().unwrap(), Duration::from_secs(5));
        assert!(!result.available);
        assert!(result.candidates.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn run_zoxide_query_times_out() {
        // A script that sleeps longer than the timeout.
        let fake = write_fake_zoxide("slow", "sleep 10");
        let start = Instant::now();
        let result = run_zoxide_query(fake.to_str().unwrap(), Duration::from_millis(200));
        let elapsed = start.elapsed();
        assert!(result.available);
        assert!(result.candidates.is_empty());
        // Should return well before the 10-second sleep finishes.
        assert!(elapsed < Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[test]
    fn run_zoxide_query_returns_empty_on_nonzero_exit() {
        let fake = write_fake_zoxide("fail", "echo '100.0 /should-not-appear' >&2; exit 1");
        let result = run_zoxide_query(fake.to_str().unwrap(), Duration::from_secs(5));
        assert!(result.available);
        assert!(result.candidates.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn run_zoxide_query_returns_empty_on_malformed_output() {
        let fake = write_fake_zoxide("malformed", "printf 'garbage no score\nbroken\n'");
        let result = run_zoxide_query(fake.to_str().unwrap(), Duration::from_secs(5));
        assert!(result.available);
        assert!(result.candidates.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn run_zoxide_query_deduplicates_canonical_aliases() {
        let dir = TestDir::new("zoxide-dedup");
        let real = dir.path().join("real");
        let alias = dir.path().join("alias");
        fs::create_dir(&real).expect("mkdir real");
        #[cfg(unix)]
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

        let result = run_zoxide_query(fake.to_str().unwrap(), Duration::from_secs(5));
        assert!(result.available);
        assert_eq!(result.candidates.len(), 1);
        // Highest-score alias wins.
        assert_eq!(result.candidates[0].canonical_path, canonical);
        assert_eq!(result.candidates[0].score, 200.0);
        assert_eq!(result.candidates[0].shown_path, real);
    }
}

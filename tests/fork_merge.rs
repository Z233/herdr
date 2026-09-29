#![cfg(unix)]

#[allow(dead_code)] // Shared black-box protocol fixture also serves other integration targets.
mod support;
// This target shares the CLI fixture, but exercises only its Unix socket helpers.
#[path = "cli/fork_merge.rs"]
mod cases;
#[allow(dead_code, unused_imports)]
#[path = "cli/harness.rs"]
mod harness;

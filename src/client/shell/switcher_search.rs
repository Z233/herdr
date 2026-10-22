//! Pure ranked Workspace → Tab → Pane projection for Workspace Switcher
//! search. Go To keeps `aggregate_navigation::navigator_rows` semantics.

use super::*;
use crate::workspace_search::match_rank;

struct SearchGroup {
    key: ((u8, usize), usize, usize, usize),
    rows: Vec<ClientNavigatorRow>,
}

fn best_rank(query: &str, fields: &[&str]) -> Option<(u8, usize)> {
    fields
        .iter()
        .filter_map(|text| match_rank(query, text))
        .min()
}

/// Ranked Switcher search rows: machine groups (direct endpoint matches),
/// workspace groups sorted by best rank, then client MRU, then endpoint and
/// snapshot order. Directories stay a separate final group. `mru_targets` is
/// the opening's frozen client workspace order (hold targets or the mobile
/// switcher order).
pub(super) fn switcher_search_rows(
    endpoints: &[ClientShellEndpoint],
    active_endpoint_id: &ClientEndpointId,
    query: &str,
    directory: &super::navigator::NavigatorDirectory,
    mru_targets: &[WorkspaceNavigationTarget],
) -> Vec<ClientNavigatorRow> {
    let federated = endpoints.len() > 1;
    let depth_offset = u8::from(federated);
    let mut groups: Vec<SearchGroup> = Vec::new();
    for (endpoint_index, endpoint) in endpoints.iter().enumerate() {
        let stale = endpoint.status != ClientEndpointStatus::Online;
        let Some(snapshot) = endpoint.snapshot.as_deref() else {
            continue;
        };
        let workspace_labels = super::sidebar::workspace_labels(snapshot);
        if let Some(machine_rank) = match_rank(query, &endpoint.label) {
            groups.push(SearchGroup {
                key: (machine_rank, usize::MAX, endpoint_index, usize::MAX),
                rows: vec![ClientNavigatorRow {
                    depth: 0,
                    label: endpoint.label.clone(),
                    meta: String::new(),
                    detail: String::new(),
                    agent: None,
                    status: None,
                    stale,
                    current: false,
                    match_rank: Some(machine_rank),
                    target: ClientNavigatorTarget::Machine {
                        endpoint_id: endpoint.endpoint_id.clone(),
                    },
                }],
            });
        }
        let agents = snapshot
            .agents
            .iter()
            .map(|agent| (agent.pane_id.as_str(), agent))
            .collect::<HashMap<_, _>>();
        let mut tabs_by_workspace: HashMap<&str, Vec<&ClientShellTab>> = HashMap::new();
        for tab in &snapshot.tabs {
            tabs_by_workspace
                .entry(tab.workspace_id.as_str())
                .or_default()
                .push(tab);
        }
        let mut panes_by_tab: HashMap<&str, Vec<&crate::protocol::ClientShellPane>> =
            HashMap::new();
        for pane in &snapshot.panes {
            panes_by_tab
                .entry(pane.tab_id.as_str())
                .or_default()
                .push(pane);
        }
        for (workspace_index, workspace) in snapshot.workspaces.iter().enumerate() {
            let label = workspace_labels
                .get(workspace.workspace_id.as_str())
                .copied()
                .unwrap_or(&workspace.label);
            let repo = workspace
                .worktree
                .as_ref()
                .map(|worktree| worktree.label.as_str());
            let mut ws_fields = vec![endpoint.label.as_str(), label, workspace.label.as_str()];
            if let Some(repo) = repo {
                ws_fields.push(repo);
            }
            if let Some(branch) = workspace.branch.as_deref() {
                ws_fields.push(branch);
            }
            // The projected workspace launch directory is a canonical match
            // field; it follows the current launch policy/foreground path and
            // is not immutable creation history. Display meta keeps its own
            // precedence.
            ws_fields.push(workspace.new_workspace_cwd.as_str());
            let ws_context = ws_fields.join(" ");
            ws_fields.push(&ws_context);
            let ws_rank = best_rank(query, &ws_fields);
            let mru = mru_targets
                .iter()
                .position(|target| {
                    target.endpoint_id == endpoint.endpoint_id
                        && target.workspace_id == workspace.workspace_id
                })
                .unwrap_or(usize::MAX);
            let workspace_tabs = tabs_by_workspace
                .get(workspace.workspace_id.as_str())
                .map(Vec::as_slice)
                .unwrap_or_default();
            let multiple_tabs = workspace_tabs.len() > 1;
            let mut group_rank = ws_rank;
            let mut included: Vec<IncludedTab> = Vec::new();
            for tab in workspace_tabs {
                let tab_context = format!("{ws_context} {}", tab.label);
                let tab_rank = best_rank(query, &[tab.label.as_str(), tab_context.as_str()]);
                let tab_matches = ws_rank.is_some() || tab_rank.is_some();
                let mut pane_rows: Vec<PaneRow> = Vec::new();
                let tab_panes = panes_by_tab
                    .get(tab.tab_id.as_str())
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                for (index, pane) in tab_panes.iter().enumerate() {
                    let agent = agents.get(pane.pane_id.as_str()).copied();
                    let label = super::aggregate_navigation::navigator_pane_label(
                        workspace,
                        tab,
                        pane,
                        agent,
                        multiple_tabs,
                        tab_panes.len(),
                        index,
                    );
                    let meta = pane.foreground_cwd.as_deref().or(pane.cwd.as_deref());
                    let agent_kind = agent.and_then(|agent| {
                        agent.agent.as_deref().or(agent.display_agent.as_deref())
                    });
                    let agent_name = agent.and_then(|agent| agent.name.as_deref());
                    let title = agent.and_then(|agent| {
                        agent
                            .title
                            .as_deref()
                            .or(agent.terminal_title_stripped.as_deref())
                    });
                    let mut pane_fields = vec![label.as_str(), pane.pane_id.as_str()];
                    let mut pane_context = format!("{tab_context} {label} {}", pane.pane_id);
                    for field in [
                        meta,
                        pane.cwd.as_deref(),
                        pane.foreground_cwd.as_deref(),
                        agent_name,
                        agent_kind,
                        title,
                    ]
                    .into_iter()
                    .flatten()
                    {
                        pane_fields.push(field);
                        pane_context.push(' ');
                        pane_context.push_str(field);
                    }
                    pane_fields.push(&pane_context);
                    let pane_rank = best_rank(query, &pane_fields);
                    if tab_matches || pane_rank.is_some() {
                        pane_rows.push(PaneRow {
                            pane,
                            agent,
                            label,
                            meta,
                            rank: pane_rank,
                            snapshot_index: index,
                        });
                    }
                }
                if tab_matches || !pane_rows.is_empty() {
                    if let Some(rank) = tab_rank {
                        group_rank =
                            Some(group_rank.map_or(rank, |best: (u8, usize)| best.min(rank)));
                    }
                    included.push((tab, tab_rank, pane_rows));
                }
            }
            if ws_rank.is_none() && included.is_empty() {
                continue;
            }
            // Tab siblings sort by their best qualifying match (tab or
            // descendant pane); the stable sort keeps snapshot order on ties.
            included.sort_by_key(|(_, tab_rank, panes)| {
                panes
                    .iter()
                    .filter_map(|pane| pane.rank)
                    .fold(*tab_rank, |best, rank| {
                        Some(best.map_or(rank, |best: (u8, usize)| best.min(rank)))
                    })
                    .unwrap_or((u8::MAX, usize::MAX))
            });
            for (_, _, panes) in &included {
                for pane in panes {
                    if let Some(rank) = pane.rank {
                        group_rank = Some(group_rank.map_or(rank, |best| best.min(rank)));
                    }
                }
            }
            let Some(group_rank) = group_rank else {
                continue;
            };
            let mut rows = vec![ClientNavigatorRow {
                depth: depth_offset,
                label: label.to_owned(),
                meta: if federated {
                    format!("{} · {}", workspace.label, endpoint.label)
                } else {
                    workspace.label.clone()
                },
                detail: workspace.new_workspace_cwd.clone(),
                agent: None,
                status: None,
                stale,
                current: false,
                match_rank: ws_rank,
                target: ClientNavigatorTarget::Workspace {
                    endpoint_id: endpoint.endpoint_id.clone(),
                    workspace_id: workspace.workspace_id.clone(),
                },
            }];
            for (tab, tab_rank, mut pane_rows) in included {
                pane_rows.sort_by_key(|pane| (pane.rank, pane.snapshot_index));
                rows.push(ClientNavigatorRow {
                    depth: 1 + depth_offset,
                    label: tab.label.clone(),
                    meta: String::new(),
                    detail: format!("{} / {}", workspace.label, tab.label),
                    agent: None,
                    status: None,
                    stale,
                    current: false,
                    match_rank: tab_rank,
                    target: ClientNavigatorTarget::Tab {
                        endpoint_id: endpoint.endpoint_id.clone(),
                        tab_id: tab.tab_id.clone(),
                    },
                });
                rows.extend(pane_rows.into_iter().map(|pane| {
                    let status = pane
                        .agent
                        .map_or(crate::api::schema::AgentStatus::Unknown, |agent| {
                            agent.agent_status
                        });
                    ClientNavigatorRow {
                        depth: 2 + depth_offset,
                        label: pane.label,
                        meta: pane.meta.unwrap_or_default().to_owned(),
                        detail: if federated {
                            format!(
                                "{} / {} / {} / {}",
                                endpoint.label, workspace.label, tab.label, pane.pane.pane_id
                            )
                        } else {
                            format!(
                                "{} / {} / {}",
                                workspace.label, tab.label, pane.pane.pane_id
                            )
                        },
                        agent: pane
                            .agent
                            .and_then(|agent| {
                                agent.agent.as_deref().or(agent.display_agent.as_deref())
                            })
                            .map(str::to_owned),
                        status: Some(status),
                        stale,
                        current: endpoint.endpoint_id == *active_endpoint_id
                            && snapshot.focused_pane_id.as_deref() == Some(&pane.pane.pane_id),
                        match_rank: pane.rank,
                        target: ClientNavigatorTarget::Pane {
                            endpoint_id: endpoint.endpoint_id.clone(),
                            pane_id: pane.pane.pane_id.clone(),
                        },
                    }
                }));
            }
            groups.push(SearchGroup {
                key: (group_rank, mru, endpoint_index, workspace_index),
                rows,
            });
        }
    }
    groups.sort_by_key(|group| group.key);
    let mut rows = groups
        .into_iter()
        .flat_map(|group| group.rows)
        .collect::<Vec<_>>();
    super::navigator::append_directory_rows(&mut rows, directory);
    rows
}

struct PaneRow<'a> {
    pane: &'a crate::protocol::ClientShellPane,
    agent: Option<&'a crate::protocol::ClientShellAgent>,
    label: String,
    meta: Option<&'a str>,
    rank: Option<(u8, usize)>,
    snapshot_index: usize,
}
/// Search auto-selection: retain a selection that still qualifies as a direct
/// match; otherwise choose the best-ranked matching target. Context-only
/// ancestors never become automatic candidates; directories are the fallback.
pub(super) fn reconcile_search_selection(
    rows: &[ClientNavigatorRow],
    selected: &mut Option<ClientNavigatorTarget>,
) {
    let still_qualifies = selected.as_ref().is_some_and(|target| {
        rows.iter()
            .any(|row| &row.target == target && row.match_rank.is_some())
    });
    if still_qualifies {
        return;
    }
    let mut best: Option<usize> = None;
    for (index, row) in rows.iter().enumerate() {
        if row.match_rank.is_none() {
            continue;
        }
        if best.is_none_or(|current| rows[current].match_rank > row.match_rank) {
            best = Some(index);
        }
    }
    *selected = best.map(|index| rows[index].target.clone()).or_else(|| {
        rows.iter()
            .find(|row| matches!(row.target, ClientNavigatorTarget::Directory { .. }))
            .map(|row| row.target.clone())
    });
}

/// First-Esc restoration target: the saved target when it survives, its
/// closest surviving valid ancestor, or the first original-list item.
pub(super) fn restored_selection(
    rows: &[ClientNavigatorRow],
    saved: Option<&ClientNavigatorTarget>,
    saved_parent: Option<&ClientNavigatorTarget>,
) -> Option<ClientNavigatorTarget> {
    let present = |target: &ClientNavigatorTarget| rows.iter().any(|row| &row.target == target);
    if let Some(target) = saved {
        if present(target) {
            return Some(target.clone());
        }
    }
    // The saved row may be gone from every snapshot; the ancestor captured at
    // search entry is the closest surviving selection.
    if let Some(parent) = saved_parent {
        if present(parent) {
            return Some(parent.clone());
        }
    }
    rows.first().map(|row| row.target.clone())
}
/// Tabs retained for one workspace group with their qualifying pane rows.
type IncludedTab<'a> = (&'a ClientShellTab, Option<(u8, usize)>, Vec<PaneRow<'a>>);

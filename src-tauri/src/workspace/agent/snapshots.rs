use std::collections::HashMap;

use super::super::Workspace;
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UsedSnapshot {
    pub project: String,
    pub recipe: String,
    pub instance_id: u64,
    pub revision: u64,
    pub round_id: Option<String>,
}

impl UsedSnapshot {
    pub(crate) fn from_workspace(project: String, workspace: &Workspace) -> Self {
        Self {
            project,
            recipe: workspace.doc.id.clone(),
            instance_id: workspace.instance_id,
            revision: workspace.revision,
            round_id: workspace
                .teaching_piece
                .as_ref()
                .map(|p| p.round_id.clone()),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CurrentSnapshot {
    pub project: String,
    pub revision: Option<u64>,
    pub instance_id: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SnapshotState {
    #[serde(flatten)]
    pub used: UsedSnapshot,
    pub stale: bool,
    pub current: CurrentSnapshot,
}

pub(crate) fn finish_batch(
    used: Vec<UsedSnapshot>,
    current_project: &str,
    workspaces: &HashMap<String, Workspace>,
) -> Vec<SnapshotState> {
    used.into_iter()
        .map(|used| {
            let current = workspaces.get(&used.recipe);
            let stale = used.project != current_project
                || current.is_none_or(|w| {
                    w.revision != used.revision || w.instance_id != used.instance_id
                });
            SnapshotState {
                used,
                stale,
                current: CurrentSnapshot {
                    project: current_project.into(),
                    revision: current.map(|w| w.revision),
                    instance_id: current.map(|w| w.instance_id),
                },
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::super::burst_tests::burst_workspace;
    use super::*;
    use crate::plan::SegmentKind::Straight;

    #[test]
    fn candidate_changes_during_later_measurement_make_earlier_batch_results_stale() {
        let mut workspace = burst_workspace(&[Straight]);
        workspace.instance_id = 42;
        let earlier = UsedSnapshot::from_workspace("project-a".into(), &workspace);
        let first_revision = workspace.revision;
        workspace.revision += 1;
        let later = UsedSnapshot::from_workspace("project-a".into(), &workspace);
        let states = finish_batch(
            vec![earlier, later],
            "project-a",
            &HashMap::from([(workspace.doc.id.clone(), workspace.clone())]),
        );
        assert!(states[0].stale);
        assert_eq!(states[0].used.revision, first_revision);
        assert_eq!(states[0].current.revision, Some(workspace.revision));
        assert!(!states[1].stale);
        assert_eq!(states[1].used.revision, workspace.revision);
    }

    #[test]
    fn recreated_candidate_with_reused_revision_cannot_supply_the_old_comparison_snapshot() {
        use super::super::super::*;
        let mut original = burst_workspace(&[Straight]);
        original.instance_id = 42;
        let host = WorkspaceHost {
            root: PathBuf::new(),
            items: Mutex::new(HashMap::from([(original.doc.id.clone(), original.clone())])),
            notes: Vec::new(),
            live_burst: Mutex::new(LiveFrames::default()),
            live_views: Mutex::new(LiveViews::default()),
            station: Mutex::new(HashMap::new()),
            capture_seq: std::sync::atomic::AtomicU64::new(0),
            comparisons: comparisons::Comparisons::default(),
            history_bundle: Mutex::new(None),
        };
        let captured = host
            .comparison_snapshot(
                &original.doc.id,
                original.revision,
                Some(original.instance_id),
            )
            .unwrap();
        let mut replacement = original.clone();
        replacement.instance_id += 1;
        replacement.doc.segments[0].checks.gap.max_gap_mm = 12.0;
        {
            let mut items = host.items.lock().unwrap();
            items.remove(&original.doc.id);
            items.insert(replacement.doc.id.clone(), replacement.clone());
        }
        assert!(host
            .comparison_snapshot(
                &captured.doc.id,
                captured.revision,
                Some(captured.instance_id)
            )
            .is_err());
        assert_eq!(
            captured.doc.segments[0].checks.gap.max_gap_mm,
            original.doc.segments[0].checks.gap.max_gap_mm
        );
        let current = host
            .comparison_snapshot(
                &replacement.doc.id,
                replacement.revision,
                Some(replacement.instance_id),
            )
            .unwrap();
        assert_eq!(current.doc.segments[0].checks.gap.max_gap_mm, 12.0);
    }
}

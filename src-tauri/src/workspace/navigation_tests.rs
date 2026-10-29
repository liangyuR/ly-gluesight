use super::burst_tests::{burst_workspace, capture_round, DEVICE};
use super::capture_review::save_template_round;
use super::navigation::{
    activate_navigation_release, edit_navigation, inherit_navigation, navigation_import_notes,
    navigation_view, prepare_navigation_release, NavigationEdit,
};
use super::pieces_tests::piece;
use super::*;
use crate::plan::SegmentKind::{Corner, Straight};
use sha2::{Digest, Sha256};

struct Project(PathBuf);

impl Project {
    fn new() -> Self {
        let path = std::env::temp_dir().join(vision::unique_run_id("gluesight-navigation-test"));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn png() -> Vec<u8> {
    let image = image::RgbaImage::from_fn(20, 10, |x, _| {
        image::Rgba([100, 120, 130, if x < 10 { 0 } else { 255 }])
    });
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}

fn mapped(project: &Path) -> Workspace {
    let mut w = burst_workspace(&[Straight, Corner]);
    w.teaching_piece = Some(piece("teach", 1));
    let round = capture_round("teach", &w.doc, 30, DEVICE, &[]);
    w = save_template_round(&w, &round, w.doc.segments.clone(), vec![15]).unwrap();
    w = edit_navigation(project, &w, w.revision, NavigationEdit::Import(png())).unwrap();
    let mut map = navigation_view(project, &w).map.unwrap();
    map.points = vec![[0.1, 0.2], [0.8, 0.2], [0.8, 0.8]];
    map.cuts = vec![0.6];
    edit_navigation(
        project,
        &w,
        w.revision,
        NavigationEdit::Save { map, confirm: true },
    )
    .unwrap()
}

fn release(project: &Path, w: &Workspace, bundle: &str) -> Release {
    let doc = formal_release_doc(w).unwrap();
    let navigation = prepare_navigation_release(project, w, &doc, bundle);
    Release {
        doc,
        bundle_id: bundle.into(),
        base_revision: w.base_revision.clone(),
        revision: w.revision,
        validation: Validation {
            revision: w.revision,
            passed: true,
            checked_at: 0,
            checks: vec![],
            samples: vec![],
            environment_tag: Value::Null,
        },
        validation_site: ValidationSite::Offline,
        navigation: Some(navigation),
        _frames: Discarded,
        _overview: Discarded,
    }
}

#[test]
fn stale_revision_or_pending_release_then_navigation_edit_is_rejected() {
    let project = Project::new();
    let w = mapped(&project.0);
    let original = w.navigation_id.clone();
    assert!(edit_navigation(&project.0, &w, w.revision - 1, NavigationEdit::Delete).is_err());
    let mut pending = w.clone();
    pending.pending = Some(Box::new(release(&project.0, &w, "release-lock")));
    assert!(edit_navigation(
        &project.0,
        &pending,
        pending.revision,
        NavigationEdit::Import(png())
    )
    .is_err());
    assert_eq!(pending.navigation_id, original);
    assert!(navigation_view(&project.0, &pending).map.unwrap().confirmed);
}

#[test]
fn invalid_spatial_partition_or_presentation_then_confirmation_cannot_replace_saved_map() {
    let project = Project::new();
    let w = mapped(&project.0);
    let original = navigation_view(&project.0, &w).map.unwrap();
    let changes: [fn(&mut NavigationMap); 8] = [
        |m| m.cuts.clear(),
        |m| m.cuts = vec![m.capture_end],
        |m| m.points[1] = m.points[0],
        |m| m.capture_end = 1.1,
        |m| m.points[0][0] = -0.1,
        |m| m.view.rotation = -181.0,
        |m| m.view.rotation = 181.0,
        |m| m.view.rotation = f64::NAN,
    ];
    for change in changes {
        let mut map = original.clone();
        change(&mut map);
        assert!(edit_navigation(
            &project.0,
            &w,
            w.revision,
            NavigationEdit::Save { map, confirm: true }
        )
        .is_err());
        assert_eq!(navigation_view(&project.0, &w).map, Some(original.clone()));
    }
}

#[test]
fn navigation_edit_then_alpha_and_detection_identity_are_preserved() {
    let project = Project::new();
    let w = mapped(&project.0);
    let before = w.doc.clone();
    let mut map = navigation_view(&project.0, &w).map.unwrap();
    let bytes = std::fs::read(
        project
            .0
            .join("workspaces")
            .join(&w.doc.id)
            .join("navigation/images")
            .join(format!("{}.png", map.image.id)),
    )
    .unwrap();
    let rgba = image::load_from_memory(&bytes).unwrap().into_rgba8();
    assert_eq!(rgba.get_pixel(0, 0)[3], 0);
    assert_eq!(rgba.get_pixel(19, 0)[3], 255);
    map.points[1][0] = 0.7;
    let edited = edit_navigation(
        &project.0,
        &w,
        w.revision,
        NavigationEdit::Save {
            map,
            confirm: false,
        },
    )
    .unwrap();
    assert_eq!(edited.doc, before);
    assert!(edited.doc.same_plan(&before));
    assert!(!navigation_view(&project.0, &edited).map.unwrap().confirmed);
    assert!(navigation_view(&project.0, &w).map.unwrap().confirmed);
}

#[test]
fn legacy_map_then_presentation_saves_and_reopens_without_changing_geometry_or_confirmation() {
    let project = Project::new();
    let mut w = mapped(&project.0);
    let original = navigation_view(&project.0, &w).map.unwrap();
    let mut legacy = serde_json::to_value(&original).unwrap();
    legacy.as_object_mut().unwrap().remove("view");
    let bytes = serde_json::to_vec(&legacy).unwrap();
    let id = format!("{:x}", Sha256::digest(&bytes));
    let maps = project
        .0
        .join("workspaces")
        .join(&w.doc.id)
        .join("navigation/maps");
    std::fs::write(maps.join(format!("{id}.json")), bytes).unwrap();
    w.navigation_id = Some(id);
    let mut map = navigation_view(&project.0, &w).map.unwrap();
    assert_eq!(map, original);
    map.view = NavigationPresentation {
        rotation: 180.0,
        flip_x: true,
        flip_y: true,
        background: NavigationBackground::Light,
    };
    let next = edit_navigation(
        &project.0,
        &w,
        w.revision,
        NavigationEdit::Save {
            map,
            confirm: original.confirmed,
        },
    )
    .unwrap();
    let workspace_file = project
        .0
        .join("workspaces")
        .join(&w.doc.id)
        .join("workspace.json");
    std::fs::write(&workspace_file, serde_json::to_vec(&next).unwrap()).unwrap();
    let reopened: Workspace =
        serde_json::from_slice(&std::fs::read(workspace_file).unwrap()).unwrap();
    let mut saved = navigation_view(&project.0, &reopened).map.unwrap();
    assert_eq!(
        saved.view,
        NavigationPresentation {
            rotation: -180.0,
            flip_x: true,
            flip_y: true,
            background: NavigationBackground::Light
        }
    );
    saved.view = NavigationPresentation::default();
    assert_eq!(saved, original);
    assert_eq!(reopened.doc, w.doc);
    assert!(reopened.doc.same_plan(&w.doc));

    legacy["view"] =
        json!({"rotation": 0, "flipX": false, "flipY": false, "background": "unsupported"});
    let bytes = serde_json::to_vec(&legacy).unwrap();
    let id = format!("{:x}", Sha256::digest(&bytes));
    std::fs::write(maps.join(format!("{id}.json")), bytes).unwrap();
    w.navigation_id = Some(id);
    let invalid = navigation_view(&project.0, &w);
    assert!(invalid.map.is_none());
    assert!(invalid.error.is_some());
    assert!(w.doc.build().is_ok());
}

#[test]
fn damaged_optional_navigation_status_then_release_remains_loadable_and_detection_fields_stay_strict(
) {
    let project = Project::new();
    let w = mapped(&project.0);
    let release = release(&project.0, &w, "release-optional-status");
    let mut file = serde_json::to_value(&release).unwrap();
    file["navigation"] = json!({ "state": ["invalid auxiliary status"] });
    let loaded: Release = serde_json::from_value(file.clone()).unwrap();
    assert!(loaded.navigation.is_none());
    assert_eq!(loaded.doc, release.doc);
    assert_eq!(loaded.bundle_id, release.bundle_id);
    file["bundleId"] = json!(["invalid detection bundle identity"]);
    assert!(serde_json::from_value::<Release>(file).is_err());
}

#[test]
fn metadata_edit_then_current_validation_stays_current_but_stale_validation_is_not_revived() {
    let project = Project::new();
    let original = mapped(&project.0);
    for current in [true, false] {
        let mut w = original.clone();
        w.validation = Some(Validation {
            revision: if current { w.revision } else { w.revision - 1 },
            passed: true,
            checked_at: 0,
            checks: vec![],
            samples: vec![],
            environment_tag: Value::Null,
        });
        let next = edit_navigation(&project.0, &w, w.revision, NavigationEdit::Delete).unwrap();
        assert_eq!(next.validation.unwrap().revision == next.revision, current);
    }
}

#[test]
fn same_count_kind_reorder_then_spatial_mapping_needs_reconfirmation() {
    let project = Project::new();
    let mut w = mapped(&project.0);
    let original_id = w.navigation_id.clone();
    let mut segments = w.doc.segments.clone();
    segments.swap(0, 1);
    for (at, segment) in segments.iter_mut().enumerate() {
        segment.index = at as u32 + 1;
    }
    super::burst::update_segments(&mut w, segments).unwrap();
    assert_eq!(w.navigation_id, original_id);
    let mut map = navigation_view(&project.0, &w).map.unwrap();
    assert!(!map.confirmed);
    map.structure_revision = w.navigation_structure_revision;
    w = edit_navigation(
        &project.0,
        &w,
        w.revision,
        NavigationEdit::Save { map, confirm: true },
    )
    .unwrap();
    let mut segments = w.doc.segments.clone();
    segments[0].tolerance += 1;
    super::burst::update_segments(&mut w, segments).unwrap();
    assert!(navigation_view(&project.0, &w).map.unwrap().confirmed);
}

#[test]
fn replacing_teaching_piece_then_map_survives_but_old_frame_range_is_not_archived() {
    let project = Project::new();
    let mut w = mapped(&project.0);
    let original = navigation_view(&project.0, &w).map.unwrap();
    super::pieces::replace_teaching_piece(&mut w, piece("new-teach", 2)).unwrap();
    assert_eq!(navigation_view(&project.0, &w).map, Some(original));
    let doc = formal_release_doc(&w).unwrap();
    let status = prepare_navigation_release(&project.0, &w, &doc, "release-new-piece");
    assert_eq!(status.state, "error");
    assert!(!project
        .0
        .join("recipe-support")
        .join(&w.doc.id)
        .join("release-new-piece")
        .exists());
}

#[test]
fn published_map_then_edits_and_corruption_do_not_mutate_snapshot_or_block_recipe_loading() {
    let project = Project::new();
    let mut w = mapped(&project.0);
    let mut map = navigation_view(&project.0, &w).map.unwrap();
    map.view = NavigationPresentation {
        rotation: 27.5,
        flip_x: true,
        flip_y: false,
        background: NavigationBackground::Checker,
    };
    w = edit_navigation(
        &project.0,
        &w,
        w.revision,
        NavigationEdit::Save { map, confirm: true },
    )
    .unwrap();
    let release = release(&project.0, &w, "release-navigation");
    assert_eq!(release.navigation.as_ref().unwrap().state, "ready");
    let final_dir = project
        .0
        .join("recipe-support")
        .join(&w.doc.id)
        .join(&release.bundle_id);
    assert!(!final_dir.exists());
    let mut mismatched = release.clone();
    mismatched.doc.segments.swap(0, 1);
    for (at, segment) in mismatched.doc.segments.iter_mut().enumerate() {
        segment.index = at as u32 + 1;
    }
    assert!(activate_navigation_release(&project.0, &mismatched).is_err());
    assert!(!final_dir.exists());
    activate_navigation_release(&project.0, &release).unwrap();
    let bytes = std::fs::read(final_dir.join("snapshot.json")).unwrap();
    let archive = project
        .0
        .join("workspaces")
        .join(&w.doc.id)
        .join("published");
    std::fs::create_dir_all(&archive).unwrap();
    let revision = release.doc.build().unwrap().revision_id;
    std::fs::write(
        archive.join(format!("{revision}.json")),
        serde_json::to_vec(&release).unwrap(),
    )
    .unwrap();
    let mut next = Workspace::new(release.doc.clone(), Some(revision));
    assert!(inherit_navigation(&project.0, &mut next).is_none());
    assert_eq!(
        navigation_view(&project.0, &next).map,
        navigation_view(&project.0, &w).map
    );
    let mut map = navigation_view(&project.0, &next).map.unwrap();
    map.points[1][0] = 0.75;
    map.view.rotation = -90.0;
    map.view.flip_y = true;
    map.view.background = NavigationBackground::Light;
    next = edit_navigation(
        &project.0,
        &next,
        next.revision,
        NavigationEdit::Save { map, confirm: true },
    )
    .unwrap();
    assert_eq!(
        std::fs::read(final_dir.join("snapshot.json")).unwrap(),
        bytes
    );
    assert!(navigation_import_notes(&project.0).is_empty());
    std::fs::write(final_dir.join("image.png"), b"damaged").unwrap();
    assert_eq!(
        navigation_view(&project.0, &next).published.unwrap().state,
        "error"
    );
    assert!(!navigation_import_notes(&project.0).is_empty());
    let store = crate::recipe::RecipeStore::open(project.0.join("recipes")).unwrap();
    let mut production = release.doc;
    production.plan_version = store.plan_version_for(&production).unwrap();
    let saved = store.save_published(production, None).unwrap();
    assert!(crate::recipe::RecipeStore::open(project.0.join("recipes"))
        .unwrap()
        .get(&saved.id)
        .is_some());
}

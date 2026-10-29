use super::burst_tests::{burst_workspace, capture_round, DEVICE};
use super::capture_review::save_template_round;
use super::navigation::{
    activate_navigation_release, edit_navigation, inherit_navigation, navigation_import_notes,
    navigation_view, prepare_navigation_release, NavigationEdit,
};
use super::pieces_tests::piece;
use super::*;
use crate::plan::SegmentKind::{Corner, Straight};

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

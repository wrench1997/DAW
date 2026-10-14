//! Save-dialog suggestions follow installed projects without changing their stored names.

use super::*;

#[test]
fn save_dialog_name_follows_real_open_project_switches() {
    let fixture = Fixture::new();
    let mut ui = UiHarness::new();
    assert_eq!(ui.app.project_save_file_name(), "Night Orchard.citrus");

    for (file_name, project_name) in [
        ("first-file.citrus", "雨后微光 — NOIRE Pure"),
        ("second-file.citrus", "Evening Sketch"),
    ] {
        let mut project = Project::blank();
        project.name = project_name.into();
        let path = fixture.0.join(file_name);
        project.save(&path).unwrap();
        ui.app
            .commit_project_intent(ProjectIntent::Open { path: path.clone() });
        assert_eq!(ui.app.saved_path.as_ref(), Some(&path));
        assert_eq!(ui.app.project.name, project_name);
        assert_eq!(
            ui.app.project_save_file_name(),
            format!("{project_name}.citrus"),
            "the suggestion must follow the newly loaded project name, not the demo or disk filename"
        );
        assert!(!ui.app.project_is_dirty_now());
    }
}

#[test]
fn new_blank_project_clears_previous_save_name() {
    let mut ui = UiHarness::new();
    let mut project = Project::blank();
    project.name = "Previous session".into();
    assert!(
        ui.app
            .finish_open_project(project, PathBuf::from("previous.citrus"))
    );
    assert_eq!(ui.app.project_save_file_name(), "Previous session.citrus");

    ui.app.commit_project_intent(ProjectIntent::New);
    assert!(ui.app.saved_path.is_none());
    assert_eq!(ui.app.project.name, "Untitled");
    assert_eq!(ui.app.project_save_file_name(), "Untitled.citrus");
    assert!(!ui.app.project_is_dirty_now());
}

#[test]
fn save_name_replaces_invalid_windows_characters_without_renaming_the_project() {
    let mut ui = UiHarness::new();
    for (project_name, expected) in [
        ("  雨后:微光/第二\\版?*  . ", "雨后_微光_第二_版__.citrus"),
        ("\t ... \r\n", "Untitled.citrus"),
        ("A\u{0}B\nC", "A_B_C.citrus"),
    ] {
        let mut project = Project::blank();
        project.name = project_name.into();
        assert!(ui.app.install_project(project, None, false));
        let before = project_fingerprint(&ui.app.project);
        assert_eq!(ui.app.project_save_file_name(), expected);
        assert_eq!(ui.app.project.name, project_name);
        assert_eq!(project_fingerprint(&ui.app.project), before);
        assert!(!ui.app.project_is_dirty_now());
    }
}

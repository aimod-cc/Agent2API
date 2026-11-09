use super::super::{registry, ConfigFormat};
use super::*;
use crate::test_support;

fn change(path: &Path, before: Option<&[u8]>, after: Option<&[u8]>) -> Change {
    if let Some(bytes) = before {
        std::fs::write(path, bytes).unwrap();
    }
    let mut change = Change::load(path.to_path_buf()).unwrap();
    change.after = after.map(<[u8]>::to_vec);
    change
}

fn plan(changes: Vec<Change>) -> clients::Plan {
    clients::Plan {
        changes,
        warnings: Vec::new(),
        profile: Profile {
            id: "test-profile".into(),
            kind: "relay".into(),
            ..Default::default()
        },
        operation: "apply".into(),
    }
}

#[test]
fn apply_then_restore_preserves_original_bytes_and_removes_created_files() {
    test_support::isolated(
        concat!(
            module_path!(),
            "::apply_then_restore_preserves_original_bytes_and_removes_created_files"
        ),
        || {
            let home = crate::paths::config_dir();
            let path = home.join("config.json");
            let extra = home.join("auth.json");
            let before = b"{\n  \"model\": \"old\", \"future\": true\n}\n";
            let plan = plan(vec![
                change(
                    &path,
                    Some(before),
                    Some(br#"{"model":"new","future":true}"#),
                ),
                change(&extra, None, Some(br#"{"api_key":"private-token"}"#)),
            ]);
            let def = registry::find("claude-code").unwrap();
            let token = preview::create_restore(def.id, &plan).unwrap()["token"]
                .as_str()
                .unwrap()
                .to_string();
            apply_plan(def, &path, &plan, &token).unwrap();
            assert_eq!(files::read(&path).unwrap(), plan.changes[0].after);
            assert!(extra.is_file());
            let view = restore_preview(def, &path, false).unwrap();
            restore(def, &path, false, view["token"].as_str().unwrap()).unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), before);
            assert!(!extra.exists());
            assert_eq!(applied_status(def.id, &path).unwrap()["canUndo"], false);
        },
    );
}

#[test]
fn external_edit_after_preview_is_preserved_and_no_other_file_is_written() {
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("first.json");
    let second = dir.path().join("second.json");
    let changes = vec![
        change(&first, Some(b"original-one"), Some(b"planned-one")),
        change(&second, Some(b"original-two"), Some(b"planned-two")),
    ];
    std::fs::write(&second, b"user-edited").unwrap();
    assert!(commit(&changes).is_err());
    assert_eq!(std::fs::read(&first).unwrap(), b"original-one");
    assert_eq!(std::fs::read(&second).unwrap(), b"user-edited");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
}

#[test]
fn restore_refuses_to_overwrite_a_new_user_edit() {
    test_support::isolated(
        concat!(
            module_path!(),
            "::restore_refuses_to_overwrite_a_new_user_edit"
        ),
        || {
            let home = crate::paths::config_dir();
            let path = home.join("config.json");
            let plan = plan(vec![change(
                &path,
                Some(br#"{"model":"old"}"#),
                Some(br#"{"model":"applied"}"#),
            )]);
            let def = registry::find("claude-code").unwrap();
            let token = preview::create_restore(def.id, &plan).unwrap()["token"]
                .as_str()
                .unwrap()
                .to_string();
            apply_plan(def, &path, &plan, &token).unwrap();
            std::fs::write(&path, br#"{"model":"user-edited"}"#).unwrap();
            assert!(restore_preview(def, &path, false).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), br#"{"model":"user-edited"}"#);
            assert_eq!(
                applied_status(def.id, &path).unwrap()["modifiedExternally"],
                true
            );
        },
    );
}

#[test]
fn rollback_restores_only_files_still_matching_the_applied_content() {
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("first.json");
    let second = dir.path().join("second.json");
    let changes = vec![
        change(&first, Some(b"original"), Some(b"applied")),
        change(&second, None, Some(b"created")),
    ];
    commit(&changes).unwrap();
    std::fs::write(&first, b"user-edited").unwrap();
    assert!(rollback(&changes).is_err());
    assert_eq!(std::fs::read(&first).unwrap(), b"user-edited");
    assert!(!second.exists());
}

#[test]
fn confirmation_cannot_be_reused_after_changing_the_plan_or_client() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let mut plan = plan(vec![change(
        &path,
        Some(b"{}"),
        Some(br#"{"model":"new"}"#),
    )]);
    let token = preview::create_restore("claude-code", &plan).unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(preview::verify(&plan, "claude-code", &token).is_ok());
    assert!(preview::verify(&plan, "codex", &token).is_err());
    plan.changes[0].after = Some(br#"{"model":"other"}"#.to_vec());
    assert!(preview::verify(&plan, "claude-code", &token).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"{}");
}

#[test]
fn jsonc_and_toml_edits_preserve_unrelated_fields_and_comments() {
    for (format, original) in [
        (ConfigFormat::Jsonc, "{\n  // keep this comment\n  \"model\": \"old\",\n  \"future\": {\"enabled\": true}\n}\n"),
        (ConfigFormat::Toml, "# keep this comment\nmodel = \"old\"\n[future]\nenabled = true\n"),
    ] {
        let mut desired = files::parse(format, original).unwrap();
        desired["model"] = json!("new");
        let rendered = files::render(format, original, &desired).unwrap();
        let text = std::str::from_utf8(&rendered).unwrap();
        assert!(text.contains("keep this comment"));
        assert_eq!(files::parse(format, text).unwrap(), desired);
        assert_eq!(desired["future"]["enabled"], true);
    }
}

#[test]
fn secret_values_do_not_leak_into_the_preview() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let plan = plan(vec![change(&path, Some(b"{}"), Some(br#"{"api_key":"private-token","nested":{"password":"private-password"},"max_tokens":123}"#))]);
    let view = preview::plan_view(&plan).unwrap().to_string();
    assert!(!view.contains("private-token"));
    assert!(!view.contains("private-password"));
    assert!(view.contains("123"));
}

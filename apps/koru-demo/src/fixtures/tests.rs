// SPDX-License-Identifier: MPL-2.0

use super::*;
fn env() -> Environment {
    Environment {
        ui_language: "en".into(),
        locale: "en-US".into(),
        hour_cycle: "h12".into(),
        time_zone: "Eastern Standard Time".into(),
    }
}
fn id(n: usize) -> String {
    format!("{n:032x}")
}
fn manifest() -> Manifest {
    Manifest::new(Path::new("D:/isolated/m3"), env()).unwrap()
}
fn paragraphs(name: &str, count: usize, base: usize) -> Vec<Paragraph> {
    let title = if name == "document-edit" {
        EDIT_TITLE
    } else {
        TASK_TITLE
    };
    std::iter::once(Paragraph {
        id: id(base + 1),
        text: title.into(),
        in_viewport: true,
    })
    .chain(
        document_text_with_count(name, count)
            .unwrap()
            .split('\n')
            .enumerate()
            .map(|(n, text)| Paragraph {
                id: id(base + n + 2),
                text: text.into(),
                in_viewport: n <= 10,
            }),
    )
    .collect()
}
#[test]
fn two_real_document_captures_produce_ten_distinct_subjects_and_no_contents() {
    let mut manifest = manifest();
    capture(
        &mut manifest,
        "document-edit",
        id(1),
        paragraphs("document-edit", 120, 1),
        None,
        None,
    )
    .unwrap();
    capture(
        &mut manifest,
        "task-reminder",
        id(1000),
        paragraphs("task-reminder", 120, 1000),
        Some("04/12/2099"),
        Some("3:30 PM"),
    )
    .unwrap();
    if let Some(edit) = &manifest.document_edit {
        assert_ne!(edit.document, edit.title);
    }
    if let Some(task) = &manifest.task_reminder {
        assert_ne!(task.document, task.title);
    }
    let json = serde_json::to_vec(&manifest).unwrap();
    assert!(
        !String::from_utf8(json.clone())
            .unwrap()
            .contains(cases::TASK_PLACEHOLDER)
    );
    assert_eq!(
        Manifest::parse(&json, Path::new("d:\\isolated\\m3")).unwrap(),
        manifest
    );
    let subjects: HashSet<_> = (1..=10)
        .map(|n| {
            format!(
                "{:?}",
                manifest.fixture("task-reminder", n).unwrap().subject
            )
        })
        .collect();
    assert_eq!(subjects.len(), 10);
}
#[test]
fn failed_or_repeated_capture_never_changes_the_prior_manifest() {
    let mut manifest = manifest();
    let before = manifest.clone();
    let mut rows = paragraphs("document-edit", 120, 1);
    rows[2].text = "personal notes".into();
    assert_eq!(
        capture(&mut manifest, "document-edit", id(1), rows, None, None),
        Err("fixture-content-or-order-mismatch")
    );
    assert_eq!(manifest, before);
    capture(
        &mut manifest,
        "document-edit",
        id(1),
        paragraphs("document-edit", 120, 1),
        None,
        None,
    )
    .unwrap();
    let before = manifest.clone();
    assert_eq!(
        capture(
            &mut manifest,
            "document-edit",
            id(1),
            paragraphs("document-edit", 120, 1),
            None,
            None
        ),
        Err("fixture-half-already-captured")
    );
    assert_eq!(manifest, before);
}
#[test]
fn owning_title_duplicate_ids_unknown_fields_and_wrong_root_are_refused() {
    let mut manifest = manifest();
    let mut rows = paragraphs("task-reminder", 120, 1);
    rows.remove(0);
    assert_eq!(
        capture(
            &mut manifest,
            "task-reminder",
            id(1),
            rows,
            Some("date"),
            Some("time")
        ),
        Err("fixture-title-mismatch")
    );
    let mut rows = paragraphs("task-reminder", 120, 1);
    rows[2].id = rows[1].id.clone();
    assert_eq!(
        capture(
            &mut manifest,
            "task-reminder",
            id(1),
            rows,
            Some("date"),
            Some("time")
        ),
        Err("fixture-duplicate-paragraph")
    );
    if let Some(edit) = &manifest.document_edit {
        assert_ne!(edit.document, edit.title);
    }
    if let Some(task) = &manifest.task_reminder {
        assert_ne!(task.document, task.title);
    }
    let json = serde_json::to_vec(&manifest).unwrap();
    assert_eq!(
        Manifest::parse(&json, Path::new("D:/other")),
        Err("fixture-root-mismatch")
    );
    let mut json: serde_json::Value = serde_json::from_slice(&json).unwrap();
    json["actions"] = serde_json::json!(["click"]);
    assert_eq!(
        Manifest::parse(
            &serde_json::to_vec(&json).unwrap(),
            Path::new("D:/isolated/m3")
        ),
        Err("fixture-manifest-invalid")
    );
}
#[test]
fn longer_fixture_is_bounded_exact_and_requires_real_offscreen_subject() {
    let mut manifest = manifest();
    let mut rows = paragraphs("document-edit", 240, 1);
    rows.last_mut().unwrap().in_viewport = true;
    assert_eq!(
        capture(&mut manifest, "document-edit", id(1), rows, None, None),
        Err("fixture-edit-visibility-invalid")
    );
    capture(
        &mut manifest,
        "document-edit",
        id(1),
        paragraphs("document-edit", 240, 1),
        None,
        None,
    )
    .unwrap();
    assert_eq!(manifest.document_edit.unwrap().filler_count, 240);
    assert!(document_text_with_count("document-edit", 241).is_err());
}
#[test]
fn environment_requires_english_ui_separately_from_os_locale() {
    let mut environment = env();
    environment.locale = "de-DE".into();
    environment.hour_cycle = "h23".into();
    assert!(environment.validate().is_ok());
    environment.ui_language = "de".into();
    assert_eq!(
        environment.validate(),
        Err("fixture-ui-language-must-be-en")
    );
}

#[test]
fn own_document_title_alias_is_valid_but_other_aliases_are_refused() {
    let mut captured = manifest();
    for (name, base) in [("document-edit", 1), ("task-reminder", 1000)] {
        let mut rows = paragraphs(name, 120, base);
        rows[0].id = id(base);
        capture(
            &mut captured,
            name,
            id(base),
            rows,
            Some("04/12/2099"),
            Some("3:30 PM"),
        )
        .unwrap();
    }
    let edit = captured.document_edit.as_ref().unwrap();
    let task = captured.task_reminder.as_ref().unwrap();
    assert_eq!(edit.document, edit.title);
    assert_eq!(task.document, task.title);
    let root = Path::new("D:/isolated/m3");
    let json = serde_json::to_vec(&captured).unwrap();
    assert_eq!(Manifest::parse(&json, root).unwrap(), captured);

    for collision in 0..6 {
        let mut invalid = captured.clone();
        let edit = invalid.document_edit.as_mut().unwrap();
        let task = invalid.task_reminder.as_mut().unwrap();
        match collision {
            0 => task.title = edit.document.clone(),
            1 => task.document = edit.document.clone(),
            2 => task.subjects[0] = task.title.clone(),
            3 => edit.subject = edit.entry.clone(),
            4 => task.subjects[1] = task.subjects[0].clone(),
            5 => edit.title = task.document.clone(),
            _ => unreachable!(),
        }
        assert_eq!(invalid.validate(root), Err("fixture-duplicate-id"));
    }
}

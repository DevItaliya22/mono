// SPDX-License-Identifier: MPL-2.0

//! Versioned fixture data, captured from two explicitly prepared UI documents.
//! No operation arrays, document creation, Store access or contents in manifests.
use crate::{
    cases,
    locator::{LocatorPattern, Object},
};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, path::Path};

pub const EDIT_TITLE: &str = "M3 document edit";
pub const TASK_TITLE: &str = "M3 task reminder";
pub const FILLERS: usize = 120;
pub const RUNS: usize = 10;
pub const MAX_MANIFEST_BYTES: u64 = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Environment {
    pub ui_language: String,
    pub locale: String,
    pub hour_cycle: String,
    pub time_zone: String,
}
impl Environment {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.ui_language != "en" {
            return Err("fixture-ui-language-must-be-en");
        }
        if !matches!(self.hour_cycle.as_str(), "h12" | "h23") {
            return Err("fixture-hour-cycle-invalid");
        }
        if [&self.locale, &self.time_zone]
            .iter()
            .any(|s| s.is_empty() || s.len() > 128 || s.chars().any(char::is_control))
        {
            return Err("fixture-environment-invalid");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentEdit {
    /// Catalog DocumentId used by Explore and document opening.
    pub document: String,
    /// Root NodeId published as the first body/title paragraph; may equal document.
    pub title: String,
    pub entry: String,
    pub subject: String,
    pub filler_count: usize,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskReminder {
    /// Catalog DocumentId used by Explore and document opening.
    pub document: String,
    /// Root NodeId published as the first body/title paragraph; may equal document.
    pub title: String,
    pub subjects: Vec<String>,
    pub reminder_date: String,
    pub reminder_time: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub version: u32,
    pub synthetic: bool,
    pub root: String,
    pub environment: Environment,
    pub document_edit: Option<DocumentEdit>,
    pub task_reminder: Option<TaskReminder>,
}
impl Manifest {
    pub fn new(root: &Path, environment: Environment) -> Result<Self, &'static str> {
        let result = Self {
            version: 1,
            synthetic: true,
            root: root.to_str().ok_or("fixture-root-unicode")?.into(),
            environment,
            document_edit: None,
            task_reminder: None,
        };
        result.validate(root)?;
        Ok(result)
    }
    pub fn parse(bytes: &[u8], root: &Path) -> Result<Self, &'static str> {
        if bytes.len() as u64 > MAX_MANIFEST_BYTES {
            return Err("fixture-manifest-too-large");
        }
        let manifest: Self =
            serde_json::from_slice(bytes).map_err(|_| "fixture-manifest-invalid")?;
        manifest.validate(root)?;
        Ok(manifest)
    }
    pub fn validate(&self, root: &Path) -> Result<(), &'static str> {
        if self.version != 1 || !self.synthetic {
            return Err("fixture-version-or-synthetic-invalid");
        }
        self.environment.validate()?;
        let expected =
            crate::input_owner::root_identity(root.to_str().ok_or("fixture-root-unicode")?)
                .map_err(|_| "fixture-root-invalid")?;
        if crate::input_owner::root_identity(&self.root).map_err(|_| "fixture-root-invalid")?
            != expected
        {
            return Err("fixture-root-mismatch");
        }
        let mut ids = HashSet::new();
        let mut insert = |id: &str| {
            model(id, "text")?;
            if !ids.insert(id.to_owned()) {
                return Err("fixture-duplicate-id");
            }
            Ok(())
        };
        if let Some(edit) = &self.document_edit {
            if !(40..=240).contains(&edit.filler_count) {
                return Err("fixture-filler-count-invalid");
            }
            insert(&edit.document)?;
            // Tree projection may derive DocumentId from its own title NodeId.
            if edit.title != edit.document {
                insert(&edit.title)?;
            }
            for id in [&edit.entry, &edit.subject] {
                insert(id)?;
            }
        }
        if let Some(task) = &self.task_reminder {
            insert(&task.document)?;
            if task.title != task.document {
                insert(&task.title)?;
            }
            if task.subjects.len() != RUNS {
                return Err("fixture-requires-ten-task-subjects");
            }
            for id in &task.subjects {
                insert(id)?;
            }
            for value in [&task.reminder_date, &task.reminder_time] {
                if value.is_empty() || value.len() > 64 || value.chars().any(char::is_control) {
                    return Err("fixture-reminder-fields-invalid");
                }
            }
        }
        Ok(())
    }
    pub fn fixture(&self, name: &str, iteration: usize) -> Result<cases::Fixture, &'static str> {
        if !(1..=RUNS).contains(&iteration) {
            return Err("fixture-iteration-invalid");
        }
        match name {
            "document-edit" => {
                let edit = self
                    .document_edit
                    .as_ref()
                    .ok_or("fixture-document-edit-missing")?;
                Ok(cases::Fixture {
                    document: model(&edit.document, "root")?,
                    entry: Some(model(&edit.entry, "text")?),
                    subject: model(&edit.subject, "text")?,
                    reminder_date: None,
                    reminder_time: None,
                })
            }
            "task-reminder" => {
                let task = self
                    .task_reminder
                    .as_ref()
                    .ok_or("fixture-task-reminder-missing")?;
                Ok(cases::Fixture {
                    document: model(&task.document, "root")?,
                    entry: None,
                    subject: model(&task.subjects[iteration - 1], "text")?,
                    reminder_date: Some(task.reminder_date.clone()),
                    reminder_time: Some(task.reminder_time.clone()),
                })
            }
            _ => Err("unknown-case"),
        }
    }
}

pub fn model(id: &str, part: &str) -> Result<LocatorPattern, &'static str> {
    if id.len() != 32
        || !id
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        || id.bytes().all(|c| c == b'0')
    {
        return Err("fixture-model-id-invalid");
    }
    LocatorPattern::parse(&format!("koru:w*/v*/n:{id}#{part}"))
        .map_err(|_| "fixture-model-id-invalid")
}
pub fn model_id(locator: &str, part: &str) -> Result<String, &'static str> {
    let parsed = LocatorPattern::parse(locator).map_err(|_| "fixture-locator-invalid")?;
    let Object::Model(id) = parsed.object else {
        return Err("fixture-locator-not-model");
    };
    if parsed.part != part {
        return Err("fixture-locator-part-invalid");
    }
    model(&id, part)?;
    Ok(id)
}

/// Contents exist only during an explicitly opted-in capture and are never logged.
pub struct Paragraph {
    pub id: String,
    pub text: String,
    /// Actual paragraph bounds overlap the current document viewport.
    pub in_viewport: bool,
}
impl std::fmt::Debug for Paragraph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Paragraph")
            .field("id", &self.id)
            .field("in_viewport", &self.in_viewport)
            .finish_non_exhaustive()
    }
}

pub fn title(name: &str) -> Result<&'static str, &'static str> {
    match name {
        "document-edit" => Ok(EDIT_TITLE),
        "task-reminder" => Ok(TASK_TITLE),
        _ => Err("unknown-case"),
    }
}

pub fn filler(index: usize) -> String {
    format!("M3 filler {index:03}")
}
pub fn document_text(name: &str) -> Result<String, &'static str> {
    document_text_with_count(name, FILLERS)
}
pub fn document_text_with_count(name: &str, count: usize) -> Result<String, &'static str> {
    if !(40..=240).contains(&count) {
        return Err("fixture-filler-count-invalid");
    }
    match name {
        "document-edit" => Ok(std::iter::once(cases::DOCUMENT_ENTRY.to_owned())
            .chain((1..=count).map(filler))
            .chain(std::iter::once(cases::DOCUMENT_BEFORE.into()))
            .collect::<Vec<_>>()
            .join("\n")),
        "task-reminder" => Ok(vec![cases::TASK_PLACEHOLDER; RUNS].join("\n")),
        _ => Err("unknown-case"),
    }
}

/// Add exactly one missing half after validating every paragraph, including the
/// required owning title. Capture is data, never a mutation plan.
pub fn capture(
    manifest: &mut Manifest,
    name: &str,
    document: String,
    paragraphs: Vec<Paragraph>,
    date: Option<&str>,
    time: Option<&str>,
) -> Result<(), &'static str> {
    model(&document, "root")?;
    let expected_title = title(name)?;
    if (name == "document-edit" && manifest.document_edit.is_some())
        || (name == "task-reminder" && manifest.task_reminder.is_some())
    {
        return Err("fixture-half-already-captured");
    }
    // DocumentId and root NodeId have distinct roles; tree projection may derive
    // the former from the latter. Neither equality nor inequality proves ownership.
    // Body::rebuild publishes that root first; the remaining rows are its contents.
    let mut paragraphs = paragraphs.into_iter();
    let title = paragraphs.next().ok_or("fixture-owning-title-missing")?;
    model(&title.id, "text")?;
    if title.text != expected_title {
        return Err("fixture-title-mismatch");
    }
    let mut ids = HashSet::from([title.id.clone()]);
    let mut body = Vec::new();
    for paragraph in paragraphs {
        model(&paragraph.id, "text")?;
        if !ids.insert(paragraph.id.clone()) {
            return Err("fixture-duplicate-paragraph");
        }
        body.push(paragraph);
    }
    let count = if name == "document-edit" {
        body.len().saturating_sub(2)
    } else {
        FILLERS
    };
    let expected = document_text_with_count(name, count)?;
    let expected: Vec<_> = expected.split('\n').collect();
    if body.len() != expected.len() || body.iter().zip(expected).any(|(p, text)| p.text != text) {
        return Err("fixture-content-or-order-mismatch");
    }
    let mut candidate = manifest.clone();
    if name == "document-edit" {
        if !body[0].in_viewport || body.last().unwrap().in_viewport {
            return Err("fixture-edit-visibility-invalid");
        }
        candidate.document_edit = Some(DocumentEdit {
            document,
            title: title.id,
            entry: body[0].id.clone(),
            subject: body.last().unwrap().id.clone(),
            filler_count: count,
        });
    } else {
        if body.iter().any(|paragraph| !paragraph.in_viewport) {
            return Err("fixture-task-visibility-invalid");
        }
        candidate.task_reminder = Some(TaskReminder {
            document,
            title: title.id,
            subjects: body.into_iter().map(|p| p.id).collect(),
            reminder_date: date.ok_or("fixture-reminder-date-required")?.into(),
            reminder_time: time.ok_or("fixture-reminder-time-required")?.into(),
        });
    }
    candidate.validate(Path::new(&candidate.root))?;
    *manifest = candidate;
    Ok(())
}

#[cfg(test)]
mod tests;

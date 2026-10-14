// SPDX-License-Identifier: MPL-2.0

//! A turn's tool rows as one collapsible box: a heading that says how long
//! the agent worked and what it did (*Worked for 28s · Ran a command ×2,
//! Used a tool*), the rows beneath it, and each row's own activity line
//! (*Started 2 minutes ago · Elapsed 1s*). Counts and times come from what
//! the chat stores — kind, status and timestamps — so the box reads the same
//! while the turn is live and when the chat is reopened.

use koru_ai_core::{AgentToolKind, ToolActivity, ToolActivityStatus};
use koru_i18n::{Key, Lang, fill, t};

use crate::agents::is_agent_row;

/// The box's heading height, and the inset of the rows beneath it.
pub(crate) const GROUP_H: f32 = 30.0;
pub(crate) const ROW_INSET: f32 = 12.0;

/// The heading: how long the turn's tools took (or that they are still
/// running), and what kinds were used, each with its count.
pub(crate) fn summary(rows: &[&ToolActivity], lang: Lang) -> String {
    let live = rows.iter().any(|row| !terminal(row.status));
    let head = if live {
        t(lang, Key::AiToolsWorking).to_owned()
    } else {
        let started = rows.iter().map(|row| row.proposed_at.millis()).min();
        let ended = rows
            .iter()
            .filter_map(|row| row.completed_at.map(|at| at.millis()))
            .max();
        match (started, ended) {
            (Some(started), Some(ended)) if ended >= started => {
                fill(t(lang, Key::AiToolsWorkedFor), &[&seconds(ended - started)])
            }
            _ => fill(t(lang, Key::AiToolsWorkedFor), &[&seconds(0)]),
        }
    };
    let mut kinds: Vec<(Key, usize)> = Vec::new();
    for row in rows {
        let key = kind_key(row);
        match kinds.iter_mut().find(|(known, _)| *known == key) {
            Some((_, count)) => *count += 1,
            None => kinds.push((key, 1)),
        }
    }
    let parts: Vec<String> = kinds
        .iter()
        .map(|(key, count)| {
            if *count > 1 {
                format!("{} ×{count}", t(lang, *key))
            } else {
                t(lang, *key).to_owned()
            }
        })
        .collect();
    if parts.is_empty() {
        head
    } else {
        format!("{head} · {}", parts.join(", "))
    }
}

/// A row's own activity line under its outcome: when it started, relative
/// to now, and how long it took once it ended.
pub(crate) fn activity_line(activity: &ToolActivity, lang: Lang, now_ms: i64) -> String {
    let started = fill(
        t(lang, Key::AiToolsStarted),
        &[&koru_i18n::time::relative_long(
            lang,
            activity.proposed_at.millis(),
            now_ms,
        )],
    );
    match activity.completed_at {
        Some(ended) => format!(
            "{started} · {}",
            fill(
                t(lang, Key::AiToolsElapsed),
                &[&seconds(ended.millis() - activity.proposed_at.millis())]
            )
        ),
        None => started,
    }
}

fn kind_key(activity: &ToolActivity) -> Key {
    if !is_agent_row(activity) {
        return Key::AiAgentUsedTool;
    }
    match AgentToolKind::parse(&activity.tool_name).unwrap_or(AgentToolKind::Other) {
        AgentToolKind::Command => Key::AiAgentRanCommand,
        AgentToolKind::FileChange => Key::AiAgentChangedFile,
        AgentToolKind::Search => Key::AiAgentSearched,
        AgentToolKind::Approval => Key::AiAgentAskedPermission,
        AgentToolKind::Tool | AgentToolKind::Mcp | AgentToolKind::Other => Key::AiAgentUsedTool,
    }
}

fn terminal(status: ToolActivityStatus) -> bool {
    matches!(
        status,
        ToolActivityStatus::Complete
            | ToolActivityStatus::Denied
            | ToolActivityStatus::Failed
            | ToolActivityStatus::Cancelled
    )
}

/// `28s`, `1m 05s`: a duration as a person reads one, under a minute in
/// whole seconds.
pub(crate) fn seconds(ms: i64) -> String {
    let total = (ms.max(0) + 500) / 1000;
    if total < 60 {
        format!("{total}s")
    } else {
        format!("{}m {:02}s", total / 60, total % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use koru_ai_core::{
        ChatId, InferenceAttemptId, MessageId, Timestamp, ToolApproval, ToolInvocationId,
    };

    fn row(
        tool_name: &str,
        status: ToolActivityStatus,
        from_ms: i64,
        to_ms: Option<i64>,
    ) -> ToolActivity {
        ToolActivity {
            id: ToolInvocationId::new(),
            attempt_id: InferenceAttemptId::new(),
            chat_id: ChatId::new(),
            assistant_message_id: MessageId::new(),
            tool_name: tool_name.into(),
            read_only: false,
            status,
            approval: ToolApproval::NotRequired,
            proposed_at: Timestamp::from_millis(from_ms),
            started_at: None,
            completed_at: to_ms.map(Timestamp::from_millis),
            result_bytes: None,
            error_code: None,
            target_workspace_id: None,
            target_document_id: None,
            error_message: None,
            agent_label: None,
            agent_summary: None,
            agent_input: None,
            agent_output: None,
            display_target: None,
            live: None,
        }
    }

    #[test]
    fn the_heading_counts_kinds_and_times_the_turn() {
        let a = row(
            "agent.command",
            ToolActivityStatus::Complete,
            1_000,
            Some(3_000),
        );
        let b = row(
            "agent.command",
            ToolActivityStatus::Complete,
            2_000,
            Some(29_000),
        );
        let c = row(
            "agent.tool",
            ToolActivityStatus::Complete,
            4_000,
            Some(5_000),
        );
        assert_eq!(
            summary(&[&a, &b, &c], Lang::En),
            "Worked for 28s · Ran a command ×2, Used a tool"
        );
        let live = row("agent.search", ToolActivityStatus::Running, 1_000, None);
        assert_eq!(
            summary(&[&a, &live], Lang::En),
            "Working… · Ran a command, Searched"
        );
        assert_eq!(seconds(65_400), "1m 05s");
    }

    #[test]
    fn a_rows_activity_line_names_its_start_and_its_length() {
        let done = row(
            "agent.command",
            ToolActivityStatus::Complete,
            1_000,
            Some(2_400),
        );
        let line = activity_line(&done, Lang::En, 1_000 + 3 * 60_000);
        assert!(line.starts_with("Started "), "{line}");
        assert!(line.ends_with("Elapsed 1s"), "{line}");
        let running = row("agent.command", ToolActivityStatus::Running, 1_000, None);
        assert!(!activity_line(&running, Lang::En, 2_000).contains("Elapsed"));
        for lang in Lang::ALL {
            assert!(!summary(&[&done], lang).is_empty());
        }
    }
}

// SPDX-License-Identifier: MPL-2.0

use rusqlite::Connection;

use super::*;

#[test]
fn fresh_database_applies_all_numbered_ai_migrations() {
    let connection = open_in_memory().unwrap();
    assert_eq!(user_version(&connection).unwrap(), SCHEMA_VERSION);

    for object in [
        "ai_settings",
        "chats",
        "messages",
        "chat_documents",
        "message_document_refs",
        "message_block_refs",
        "message_blocks",
        "message_blocks_fts",
        "inference_attempts",
        "feedback_events",
        "document_exports",
        "chat_workspace_grants",
        "tool_invocations",
        "message_citations",
        "current_message_feedback",
        "inference_attempt_durations",
        "usage_by_day",
        "response_quality_by_model",
    ] {
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = ?1)",
                [object],
                |row| row.get(0),
            )
            .unwrap();
        assert!(exists, "missing schema object {object}");
    }
}

#[test]
fn migration_is_idempotent() {
    let mut connection = open_in_memory().unwrap();
    prepare(&mut connection).unwrap();
    assert_eq!(user_version(&connection).unwrap(), SCHEMA_VERSION);
}

#[test]
fn the_v0_fixture_upgrades_through_production_migration_code() {
    let mut connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(include_str!("../fixtures/ai-v0.sql"))
        .unwrap();
    assert_eq!(user_version(&connection).unwrap(), 0);

    prepare(&mut connection).unwrap();
    assert_eq!(user_version(&connection).unwrap(), SCHEMA_VERSION);
    let chats: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = 'chats')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(chats);
}

#[test]
fn the_v0_fixture_gains_a_null_working_folder_for_every_chat() {
    let mut connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(include_str!("../fixtures/ai-v0.sql"))
        .unwrap();
    prepare(&mut connection).unwrap();
    let has_column: bool = connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM pragma_table_info('chats') WHERE name = 'agent_working_folder'
             )",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(has_column, "schema 7 adds the working folder column");
    let all_null: bool = connection
        .query_row(
            "SELECT NOT EXISTS(SELECT 1 FROM chats WHERE agent_working_folder IS NOT NULL)",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(all_null, "every earlier chat keeps the home-folder default");
}

#[test]
fn the_v0_fixture_gains_null_agent_words_on_every_tool_row() {
    let mut connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(include_str!("../fixtures/ai-v0.sql"))
        .unwrap();
    prepare(&mut connection).unwrap();
    for column in [
        "agent_label",
        "agent_summary",
        "agent_input",
        "agent_output",
    ] {
        let has_column: bool = connection
            .query_row(
                &format!(
                    "SELECT EXISTS(
                        SELECT 1 FROM pragma_table_info('tool_invocations') WHERE name = '{column}'
                     )"
                ),
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(has_column, "schema 8 adds {column}");
        let all_null: bool = connection
            .query_row(
                &format!(
                    "SELECT NOT EXISTS(SELECT 1 FROM tool_invocations WHERE {column} IS NOT NULL)"
                ),
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(all_null, "every earlier row reads by kind and outcome");
    }
    // The ceiling holds: a label past it is refused, not truncated.
    let refused = connection.execute(
        "UPDATE tool_invocations SET agent_label = ?1",
        [&"x".repeat(1_001)],
    );
    assert!(
        refused.is_err() || refused.unwrap() == 0,
        "the column's check is the ceiling"
    );
}

#[test]
fn a_v1_database_preserves_rows_and_adds_message_block_provenance() {
    let mut connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(include_str!(
            "../../migrations/001_ai_initial_migration.sql"
        ))
        .unwrap();
    connection.pragma_update(None, "user_version", 1).unwrap();
    connection
        .execute(
            "INSERT INTO chats (
                id, title, current_provider, current_model, created_at_ms, updated_at_ms
             ) VALUES ('chat', 'kept', 'mock', 'model', 1, 1)",
            [],
        )
        .unwrap();

    prepare(&mut connection).unwrap();

    assert_eq!(user_version(&connection).unwrap(), SCHEMA_VERSION);
    let title: String = connection
        .query_row("SELECT title FROM chats WHERE id = 'chat'", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(title, "kept");
    let provenance_exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = 'message_block_refs')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(provenance_exists);
}

#[test]
fn a_v3_database_preserves_chats_and_defaults_them_to_public() {
    let mut connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(include_str!(
            "../../migrations/001_ai_initial_migration.sql"
        ))
        .unwrap();
    connection
        .execute_batch(include_str!(
            "../../migrations/002_ai_context_refs_migration.sql"
        ))
        .unwrap();
    connection
        .execute_batch(include_str!(
            "../../migrations/003_ai_workspace_tools_migration.sql"
        ))
        .unwrap();
    connection.pragma_update(None, "user_version", 3).unwrap();
    connection
        .execute(
            "INSERT INTO chats (
                id, title, current_provider, current_model, created_at_ms, updated_at_ms
             ) VALUES ('chat', 'kept', 'mock', 'model', 1, 1)",
            [],
        )
        .unwrap();

    prepare(&mut connection).unwrap();

    assert_eq!(user_version(&connection).unwrap(), SCHEMA_VERSION);
    let (title, private): (String, bool) = connection
        .query_row(
            "SELECT title, private FROM chats WHERE id = 'chat'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(title, "kept");
    assert!(!private);
}

#[test]
fn a_v4_database_adds_search_accounting_and_citations_without_changing_rows() {
    let mut connection = Connection::open_in_memory().unwrap();
    for migration in [
        include_str!("../../migrations/001_ai_initial_migration.sql"),
        include_str!("../../migrations/002_ai_context_refs_migration.sql"),
        include_str!("../../migrations/003_ai_workspace_tools_migration.sql"),
        include_str!("../../migrations/004_ai_chat_privacy_migration.sql"),
    ] {
        connection.execute_batch(migration).unwrap();
    }
    connection.pragma_update(None, "user_version", 4).unwrap();
    connection
        .execute(
            "INSERT INTO chats (
                id, title, current_provider, current_model, created_at_ms, updated_at_ms
             ) VALUES ('chat', 'kept', 'openai', 'gpt-5.4', 1, 1)",
            [],
        )
        .unwrap();

    prepare(&mut connection).unwrap();

    assert_eq!(user_version(&connection).unwrap(), SCHEMA_VERSION);
    let title: String = connection
        .query_row("SELECT title FROM chats WHERE id = 'chat'", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(title, "kept");
    for column in ["web_search_requested", "web_search_calls"] {
        let exists = connection
            .prepare("PRAGMA table_info(inference_attempts)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .iter()
            .any(|name| name == column);
        assert!(exists, "missing inference_attempts.{column}");
    }
}

#[test]
fn a_v5_database_adds_minimal_tool_context_columns() {
    let mut connection = Connection::open_in_memory().unwrap();
    for migration in [
        include_str!("../../migrations/001_ai_initial_migration.sql"),
        include_str!("../../migrations/002_ai_context_refs_migration.sql"),
        include_str!("../../migrations/003_ai_workspace_tools_migration.sql"),
        include_str!("../../migrations/004_ai_chat_privacy_migration.sql"),
        include_str!("../../migrations/005_ai_web_search_migration.sql"),
    ] {
        connection.execute_batch(migration).unwrap();
    }
    connection.pragma_update(None, "user_version", 5).unwrap();

    prepare(&mut connection).unwrap();

    assert_eq!(user_version(&connection).unwrap(), SCHEMA_VERSION);
    let columns = connection
        .prepare("PRAGMA table_info(tool_invocations)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    for column in ["target_workspace_id", "target_document_id", "error_message"] {
        assert!(columns.iter().any(|candidate| candidate == column));
    }
}

#[test]
fn newer_schema_is_refused_before_persistent_pragmas_or_rows_are_changed() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ai.db");
    {
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(&format!(
                "PRAGMA journal_mode = DELETE;
                 CREATE TABLE sentinel (value TEXT NOT NULL);
                 INSERT INTO sentinel VALUES ('untouched');
                 PRAGMA user_version = {};",
                SCHEMA_VERSION + 1
            ))
            .unwrap();
    }

    assert_eq!(
        open(&path).unwrap_err(),
        OpenError::SchemaTooNew {
            database: crate::DATABASE_FILE,
            discovered: SCHEMA_VERSION + 1,
            supported: SCHEMA_VERSION,
        }
    );

    let connection = Connection::open(&path).unwrap();
    let journal: String = connection
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();
    let sentinel: String = connection
        .query_row("SELECT value FROM sentinel", [], |row| row.get(0))
        .unwrap();
    assert_eq!(journal, "delete");
    assert_eq!(sentinel, "untouched");
}

#[test]
fn a_real_database_is_wal_full_and_foreign_keyed() {
    let directory = tempfile::tempdir().unwrap();
    let connection = open(&directory.path().join("nested").join("ai.db")).unwrap();

    let journal: String = connection
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();
    let synchronous: i64 = connection
        .query_row("PRAGMA synchronous", [], |row| row.get(0))
        .unwrap();
    let foreign_keys: bool = connection
        .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
        .unwrap();

    assert_eq!(journal, "wal");
    assert_eq!(synchronous, 2, "durable chat commits require FULL");
    assert!(foreign_keys);
}

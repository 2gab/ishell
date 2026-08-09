use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, named_params};

/// The Current Database schema version this application is meant to run against
pub(super) const DB_VERSION: u32 = 2;

/// Get the declared type of `column` in `table`, or `None` if the column does not exist.
///
/// Used to make migrations idempotent: `user_version` and the actual schema can end up out
/// of sync (e.g. a process killed after a migration's `ALTER TABLE`s ran but before the
/// `user_version` bump committed — see the OOM-prone Termux install path), and a plain
/// `ALTER TABLE ... ADD COLUMN` is not safe to just re-run in that case.
fn column_type(conn: &Connection, table: &str, column: &str) -> Result<Option<String>> {
    let query = format!("SELECT type FROM pragma_table_info('{table}') WHERE name = ?1");
    conn.query_row(&query, [column], |r| r.get(0))
        .optional()
        .with_context(|| format!("get column type for \"{table}.{column}\""))
}

/// Whether `column` exists on `table`.
fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    Ok(column_type(conn, table, column)?.is_some())
}

/// Helper function to get the `user_version` with a single function call.
#[inline]
fn get_user_version(conn: &Connection) -> Result<u32> {
    conn.query_row("SELECT user_version FROM pragma_user_version", [], |r| {
        r.get(0)
    })
    .context("get pragma \"user_version\"")
}

/// Helper function to set the `user_version` with a single function call.
///
/// Returns the passed version for re-use.
#[inline]
fn set_user_version(conn: &Connection, version: u32) -> Result<u32> {
    conn.pragma_update(None, "user_version", version)
        .context("update user_version error")?;

    Ok(version)
}

/// Check and update the database to be at [`DB_VERSION`].
pub(super) fn migrate(conn: &Connection) -> Result<()> {
    let user_version: u32 = get_user_version(conn)?;

    if user_version > DB_VERSION {
        bail!(
            "Expected Database version to be lower or equal to {DB_VERSION}, found {user_version}!"
        );
    }

    // only execute migrations if not already done so
    if user_version != DB_VERSION {
        apply_migrations(conn, user_version)?;
    }

    Ok(())
}

/// Apply migrations to be at [`DB_VERSION`].
#[allow(unused_assignments)] // for future possible migrations
fn apply_migrations(conn: &Connection, mut user_version: u32) -> Result<()> {
    if user_version == 0 {
        // Version 2 is the base version, so there are basically no migrations, only creations
        conn.execute_batch(include_str!("./migrations/001.sql"))
            .context("Database version 1 could not be created")?;
        user_version = set_user_version(conn, 1)?;

        set_db_created_at(conn)?;
        set_db_created_with(conn)?;
    }

    if user_version == 1 {
        // Add total_play_count and last_played_at columns for sort support (MostPlayed, Recency, Frecency) plus `added_at` column type change
        let tx = conn.unchecked_transaction()?;
        migrate_v1_to_v2(&tx).context("Database version 2 migration failed")?;
        set_user_version(&tx, 2)?;
        tx.commit()?;
    }

    set_last_updated_at(conn)?;

    Ok(())
}

/// Migrate the `tracks` table schema from version 1 to version 2. Every step checks the
/// current schema first so that re-running this against a database that already has some or
/// all of these changes (schema ahead of `user_version`) is a no-op rather than an
/// `ALTER TABLE` error.
fn migrate_v1_to_v2(conn: &Connection) -> Result<()> {
    // Total number of times the track has been started (incremented once per start).
    if !column_exists(conn, "tracks", "total_play_count")? {
        conn.execute_batch(
            "ALTER TABLE tracks ADD COLUMN total_play_count INTEGER NOT NULL DEFAULT 0;",
        )?;
    }

    // Unix epoch seconds of the last time the track was started. NULL if never played.
    if !column_exists(conn, "tracks", "last_played_at")? {
        conn.execute_batch("ALTER TABLE tracks ADD COLUMN last_played_at INTEGER;")?;
    }

    // added_at was originally DATE (RFC 3339 strings) but we now need INTEGER (unix epoch
    // seconds) for numeric sorting. SQLite does not support altering a column type in
    // place, so drop and re-create it. Use the column's declared type (rather than mere
    // presence, since "added_at" exists in both the old and new schema) to tell whether
    // this step already ran.
    if column_type(conn, "tracks", "added_at")?.as_deref() != Some("INTEGER") {
        if !column_exists(conn, "tracks", "added_at_new")? {
            conn.execute_batch("ALTER TABLE tracks ADD COLUMN added_at_new INTEGER;")?;
            // Preserve existing values by converting the old DATE string to unix epoch.
            conn.execute_batch(
                "UPDATE tracks SET added_at_new = CAST(strftime('%s', added_at) AS INTEGER);",
            )?;
        }
        conn.execute_batch("ALTER TABLE tracks DROP COLUMN added_at;")?;
        conn.execute_batch("ALTER TABLE tracks RENAME COLUMN added_at_new TO added_at;")?;
    }

    Ok(())
}

// the following are to set some values in table "config", values which could help debugging database issues.

/// Set database config value `last_migrated_at` to the current time.
#[inline]
fn set_last_updated_at(conn: &Connection) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();

    conn.execute(
        "INSERT INTO config(key, value) VALUES (\"last_migrated_at\", :value)
            ON CONFLICT(key) DO UPDATE SET value=excluded.value;",
        named_params! {":value": now},
    )?;

    Ok(())
}

/// Set database config value `db_created_at` to the current time.
#[inline]
fn set_db_created_at(conn: &Connection) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();

    conn.execute(
        "INSERT INTO config(key, value) VALUES (\"db_created_at\", :value) ON CONFLICT(key) DO NOTHING;",
        named_params! {":value": now},
    )?;

    Ok(())
}

/// Set database config value `db_created_with` to the current time.
#[inline]
fn set_db_created_with(conn: &Connection) -> Result<()> {
    let version = crate::VERSION;

    conn.execute(
        "INSERT INTO config(key, value) VALUES (\"db_created_with\", :value) ON CONFLICT(key) DO NOTHING;",
        named_params! {":value": version},
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use crate::new_database::migrate::{DB_VERSION, get_user_version, migrate, set_user_version};

    use super::super::test_utils::gen_database_raw;

    #[test]
    fn should_create_from_fresh() {
        let conn = gen_database_raw();

        // verify the created database is at 0
        assert_eq!(0, get_user_version(&conn).unwrap());
        migrate(&conn).unwrap();
        // verify the migrated database is at the highest version we want to work with
        assert_eq!(DB_VERSION, get_user_version(&conn).unwrap());

        // verify it has all the tables we expect
        let mut all_tracks: Vec<String> = {
            let mut prep = conn.prepare("SELECT name FROM sqlite_schema WHERE type ='table' AND name NOT LIKE 'sqlite_%';").unwrap();
            prep.query_map([], |r| r.get(0))
                .unwrap()
                .flatten()
                .collect()
        };

        all_tracks.sort();

        let expected = {
            let mut orig = [
                "config",
                "tracks",
                "tracks_metadata",
                "artists",
                "tracks_artists",
                "albums",
                "albums_artists",
            ];

            #[allow(clippy::stable_sort_primitive)]
            orig.sort();
            orig
        };

        assert_eq!(&all_tracks, &expected);
    }

    #[test]
    fn should_tolerate_schema_ahead_of_user_version() {
        let conn = gen_database_raw();

        // fully migrate once, then rewind `user_version` without touching the schema, to
        // simulate a database where migration 002's ALTER TABLEs already landed but the
        // user_version bump did not (e.g. the process got killed in between).
        migrate(&conn).unwrap();
        assert_eq!(DB_VERSION, get_user_version(&conn).unwrap());
        set_user_version(&conn, 1).unwrap();

        // re-running the migration against that already-migrated schema must not fail
        migrate(&conn).unwrap();
        assert_eq!(DB_VERSION, get_user_version(&conn).unwrap());
    }
}

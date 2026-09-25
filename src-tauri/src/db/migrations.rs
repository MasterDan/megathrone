use std::io;
use std::path::Path;

/// A single versioned migration, loaded from a `vN.name.sql` file in
/// `src-tauri/migrations/`.
pub struct Migration {
    pub version: i64,
    pub name: String,
    pub sql: String,
}

/// The ordered set of available migrations (ascending version, versions
/// unique; gaps in the numbering are fine — the runner simply applies the
/// next file past the database's current version).
pub struct Migrations {
    entries: Vec<Migration>,
}

// The compile-time copies of the migration files. The folder ships as a
// Tauri resource (bundle.resources) and is loaded from the resource dir at
// runtime — but Android cannot serve resource files to Rust (resource_dir()
// is the virtual asset://localhost/ URI, not a filesystem path), so these
// embedded copies are the fallback there (and what unit tests run). The
// `migration_files_match_the_embedded_set` test keeps the two in sync.
const EMBEDDED_MIGRATIONS: &[(i64, &str, &str)] = &[
    (
        1,
        "baseline",
        include_str!("../../migrations/v1.baseline.sql"),
    ),
    (
        8,
        "category_actions",
        include_str!("../../migrations/v8.category_actions.sql"),
    ),
    (
        9,
        "typed_routing_rules",
        include_str!("../../migrations/v9.typed_routing_rules.sql"),
    ),
    (
        10,
        "selection_mode",
        include_str!("../../migrations/v10.selection_mode.sql"),
    ),
    (
        11,
        "catalog_domain_suffixes",
        include_str!("../../migrations/v11.catalog_domain_suffixes.sql"),
    ),
    (
        12,
        "discovery_sources",
        include_str!("../../migrations/v12.discovery_sources.sql"),
    ),
];

impl Migrations {
    /// The compile-time copy of `src-tauri/migrations/` — the fallback when
    /// the bundled resource folder is unavailable (Android) and what unit
    /// tests exercise.
    pub fn embedded() -> Self {
        Self {
            entries: EMBEDDED_MIGRATIONS
                .iter()
                .map(|&(version, name, sql)| Migration {
                    version,
                    name: name.to_string(),
                    sql: sql.to_string(),
                })
                .collect(),
        }
    }

    /// Loads the migrations bundled at `dir` (the Tauri resource folder).
    /// Files not matching the `vN.name.sql` pattern are skipped as noise;
    /// a duplicate version is a packaging bug and errors out.
    pub fn from_dir(dir: &Path) -> io::Result<Self> {
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let Some(file_name) = entry.file_name().into_string().ok() else {
                continue;
            };
            let Some((version, name)) = parse_migration_file_name(&file_name) else {
                continue;
            };
            entries.push(Migration {
                version,
                name: name.to_string(),
                sql: std::fs::read_to_string(entry.path())?,
            });
        }
        entries.sort_by_key(|migration| migration.version);
        for pair in entries.windows(2) {
            if pair[0].version == pair[1].version {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "duplicate migration version {} ({} / {})",
                        pair[0].version, pair[0].name, pair[1].name
                    ),
                ));
            }
        }
        Ok(Self { entries })
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The highest version in this set — a resource-dir copy whose latest
    /// trails the embedded chain is stale (the dev CLI does not always
    /// refresh already-copied resources) and must not pin the schema below
    /// what the running binary knows.
    pub fn latest_version(&self) -> i64 {
        self.entries.last().map(|m| m.version).unwrap_or(0)
    }

    /// The entry with exactly this version (the heal wants the v1 baseline).
    pub(super) fn find(&self, version: i64) -> Option<&Migration> {
        self.entries.iter().find(|m| m.version == version)
    }

    /// The next entry past `version`, if any.
    pub(super) fn after(&self, version: i64) -> Option<&Migration> {
        self.entries.iter().find(|m| m.version > version)
    }
}

/// The highest version in the embedded set — what a fully-migrated
/// database carries in `PRAGMA user_version`.
#[cfg(test)]
pub(super) fn latest_version() -> i64 {
    Migrations::embedded().latest_version()
}

/// `("v8.category_actions.sql",) -> Some((8, "category_actions"))`
fn parse_migration_file_name(file_name: &str) -> Option<(i64, &str)> {
    let stem = file_name.strip_suffix(".sql")?;
    let rest = stem.strip_prefix('v')?;
    let (version, name) = rest.split_once('.')?;
    let version: i64 = version.parse().ok()?;
    (version > 0).then_some((version, name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::runner::{column_exists, open, table_exists};
    use crate::db::settings::get_setting;

    /// A fresh database replays the whole file chain and lands on the
    /// current schema: every table present, the legacy tables the v8 rework
    /// absorbed gone, settings seeded, user_version at the latest file.
    #[test]
    fn fresh_database_reaches_latest_version() {
        let path = std::env::temp_dir().join(format!(
            "megathrone-db-fresh-{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);

        let conn = open(&path, &Migrations::embedded()).unwrap();

        for table in [
            "profiles",
            "endpoints",
            "dpi_strategies",
            "app_settings",
            "test_site_categories",
            "test_sites",
            "discovery_sources",
            "endpoint_url_results",
            "dpi_url_results",
        ] {
            assert!(table_exists(&conn, table).unwrap(), "missing {table}");
        }
        for legacy in ["test_urls", "route_rules"] {
            assert!(!table_exists(&conn, legacy).unwrap(), "{legacy} should be gone");
        }
        assert!(column_exists(&conn, "profiles", "selection_mode").unwrap());
        assert!(column_exists(&conn, "test_sites", "rule_type").unwrap());
        assert_eq!(get_setting(&conn, "dpi_port").unwrap().as_deref(), Some("1080"));
        assert_eq!(get_setting(&conn, "route_fallback").unwrap().as_deref(), Some("proxy"));

        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, latest_version());
    }

    /// The bundled resource folder and the compile-time embedded set must
    /// carry the exact same files — a migration added without its
    /// `include_str!` entry would silently skip Android.
    #[test]
    fn migration_files_match_the_embedded_set() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
        let from_disk = Migrations::from_dir(&dir).expect("migrations folder should load");
        let embedded = Migrations::embedded();

        let disk: Vec<(i64, String, String)> = from_disk
            .entries
            .iter()
            .map(|m| (m.version, m.name.clone(), m.sql.clone()))
            .collect();
        let embedded: Vec<(i64, String, String)> = embedded
            .entries
            .iter()
            .map(|m| (m.version, m.name.clone(), m.sql.clone()))
            .collect();
        assert_eq!(disk, embedded, "register new files in EMBEDDED_MIGRATIONS");
        assert!(!disk.is_empty());
    }
}

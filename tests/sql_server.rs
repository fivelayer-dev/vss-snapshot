//! Backing up and restoring a SQL Server database through its VSS writer.
//!
//! These run only in the SQL Server workflow on GitHub Actions, which installs SQL Server
//! Express, names its instance in `VSS_SNAPSHOT_TEST_SQL_INSTANCE`, and makes two test
//! volumes, named in `VSS_SNAPSHOT_TEST_VOLUMES`, with a `sql` folder on each that SQL Server
//! may write to. Everywhere else each test prints `SKIPPED` and passes.
#![cfg(windows)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use vss_snapshot::{
    list_writers, BackupDocument, Component, ComponentId, Error, Guid, Restore, RestoreOptions,
    SnapshotOptions, SnapshotSet,
};

/// SQL Server's VSS writer, the same on every machine.
const SQL_WRITER: Guid = Guid::from_u128(0xa65faa63_5ea8_4ebc_9dbd_a0c4db26912a);
const TIMEOUT: Duration = Duration::from_secs(300);
const ROWS: u64 = 100;

static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

struct Sql {
    _turn: MutexGuard<'static, ()>,
    instance: String,
    first: String,
    second: String,
}

fn sql(test: &str) -> Option<Sql> {
    if std::env::var_os("GITHUB_ACTIONS").is_none() {
        eprintln!("SKIPPED {test}: real snapshots are only taken on GitHub Actions");
        return None;
    }
    let Ok(instance) = std::env::var("VSS_SNAPSHOT_TEST_SQL_INSTANCE") else {
        eprintln!(
            "SKIPPED {test}: only the SQL Server workflow sets VSS_SNAPSHOT_TEST_SQL_INSTANCE"
        );
        return None;
    };
    let volumes = std::env::var("VSS_SNAPSHOT_TEST_VOLUMES")
        .expect("the workflow should set VSS_SNAPSHOT_TEST_VOLUMES");
    let mut volumes = volumes.split(',').map(str::to_owned);
    let (Some(first), Some(second)) = (volumes.next(), volumes.next()) else {
        panic!("VSS_SNAPSHOT_TEST_VOLUMES should name two volumes");
    };
    Some(Sql {
        _turn: ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner),
        instance,
        first,
        second,
    })
}

impl Sql {
    /// Runs a query with `sqlcmd`, and returns what it printed.
    fn query(&self, query: &str) -> String {
        self.try_query(query)
            .unwrap_or_else(|error| panic!("{error}"))
    }

    /// Like `query`, for cleanup, which mustn't panic while a failed test is unwinding.
    fn try_query(&self, query: &str) -> Result<String, String> {
        let output = Command::new("sqlcmd")
            .args([
                "-S",
                &self.instance,
                "-E",
                "-C",
                "-b",
                "-h",
                "-1",
                "-W",
                "-Q",
                query,
            ])
            .output()
            .map_err(|error| format!("sqlcmd should be on the PATH: {error}"))?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        if output.status.success() {
            Ok(stdout)
        } else {
            Err(format!(
                "{query}\n{stdout}\n{}",
                String::from_utf8_lossy(&output.stderr)
            ))
        }
    }

    fn count(&self, database: &str) -> u64 {
        let output = self.query(&format!(
            "SET NOCOUNT ON; SELECT COUNT(*) FROM [{database}].dbo.t"
        ));
        output.trim().parse().unwrap()
    }

    fn state(&self, database: &str) -> String {
        self.query(&format!(
            "SET NOCOUNT ON; SELECT state_desc FROM sys.databases WHERE name = '{database}'"
        ))
        .trim()
        .to_owned()
    }

    fn insert(&self, database: &str, rows: u64) {
        self.query(&format!(
            "INSERT INTO [{database}].dbo.t (v) SELECT TOP ({rows}) 'row' FROM sys.all_objects"
        ));
    }
}

/// A test database whose data file is on `data` and log on `log`, with `ROWS` rows in it.
/// Dropped, with its files, afterwards.
struct Database<'a> {
    sql: &'a Sql,
    name: String,
    backup: PathBuf,
}

impl<'a> Database<'a> {
    fn create(sql: &'a Sql, name: &str, data: &str, log: &str) -> Database<'a> {
        let database = Database {
            sql,
            name: name.to_owned(),
            backup: std::env::temp_dir().join("vss-snapshot-sql").join(name),
        };
        database.drop_if_there();
        sql.query(&format!(
            "CREATE DATABASE [{name}] \
             ON (NAME = [{name}_data], FILENAME = '{data}\\sql\\{name}.mdf') \
             LOG ON (NAME = [{name}_log], FILENAME = '{log}\\sql\\{name}_log.ldf')"
        ));
        sql.query(&format!(
            "CREATE TABLE [{name}].dbo.t (id INT IDENTITY PRIMARY KEY, v NVARCHAR(50))"
        ));
        sql.insert(name, ROWS);
        let _ = fs::remove_dir_all(&database.backup);
        fs::create_dir_all(&database.backup).unwrap();
        database
    }

    /// The query that drops the database. Only an online database can be altered, and a
    /// failed restore can leave it `RESTORING`.
    fn drop_query(&self) -> String {
        let name = &self.name;
        format!(
            "IF DB_ID('{name}') IS NOT NULL BEGIN \
             IF DATABASEPROPERTYEX('{name}', 'Status') = 'ONLINE' \
             ALTER DATABASE [{name}] SET SINGLE_USER WITH ROLLBACK IMMEDIATE; \
             DROP DATABASE [{name}]; END"
        )
    }

    fn drop_if_there(&self) {
        self.sql.query(&self.drop_query());
    }

    /// The database's component, from the SQL Server writer.
    fn component(&self) -> (ComponentId, Component) {
        let writers = list_writers(TIMEOUT).unwrap();
        let writer = writers
            .iter()
            .find(|w| w.id == SQL_WRITER)
            .expect("the SQL Server writer should be listed");
        let component = writer
            .components
            .iter()
            .find(|c| c.name == self.name)
            .unwrap_or_else(|| panic!("no component for {}: {writer:?}", self.name))
            .clone();
        (writer.component_id(&component), component)
    }

    /// Backs the database up through its writer, copying its files out of the snapshot,
    /// then adds rows that the backup doesn't have.
    fn back_up(&self) -> (ComponentId, Component, BackupDocument) {
        let (id, component) = self.component();
        let volumes = component.volumes().unwrap();
        let options = SnapshotOptions::new()
            .with_timeout(TIMEOUT)
            .with_components([id.clone()]);
        let snapshots = SnapshotSet::create(volumes, &options).unwrap();
        for file in files(&component) {
            let source = snapshots.path_in_snapshot(&file).unwrap();
            fs::copy(&source, self.backup.join(file.file_name().unwrap())).unwrap();
        }
        let document = snapshots.complete().unwrap();
        assert!(!document.as_xml().is_empty());
        self.sql.insert(&self.name, 5);
        (id, component, document)
    }

    /// Copies the backed-up files back to where they were.
    fn copy_back(&self, component: &Component) {
        for file in files(component) {
            fs::copy(self.backup.join(file.file_name().unwrap()), &file).unwrap();
        }
    }
}

impl Drop for Database<'_> {
    fn drop(&mut self) {
        let _ = self.sql.try_query(&self.drop_query());
        let _ = fs::remove_dir_all(&self.backup);
    }
}

/// All of the component's files. SQL Server names each file exactly, and lists a database's
/// data and log files together in `files`.
fn files(component: &Component) -> Vec<PathBuf> {
    let files: Vec<PathBuf> = component
        .files
        .iter()
        .chain(&component.database_files)
        .chain(&component.log_files)
        .map(|spec| {
            assert!(!spec.filespec.contains('*'), "{spec:?}");
            Path::new(&spec.path).join(&spec.filespec)
        })
        .collect();
    assert!(!files.is_empty(), "no files in {component:?}");
    files
}

fn has_extension(files: &[PathBuf], extension: &str) -> bool {
    files.iter().any(|file| {
        file.extension()
            .is_some_and(|e| e.eq_ignore_ascii_case(extension))
    })
}

#[test]
fn the_sql_server_writer_lists_a_database_and_its_volumes() {
    let Some(sql) = sql("the_sql_server_writer_lists_a_database_and_its_volumes") else {
        return;
    };
    let database = Database::create(&sql, "vsslist", &sql.first, &sql.second);
    let (id, component) = database.component();
    eprintln!("{id}\n{component:?}");
    assert_eq!(id.writer_id, SQL_WRITER);
    let listed = files(&component);
    assert!(has_extension(&listed, "mdf"), "{listed:?}");
    assert!(has_extension(&listed, "ldf"), "{listed:?}");
    assert_eq!(
        component.volumes().unwrap(),
        [format!(r"{}\", sql.first), format!(r"{}\", sql.second)]
    );
}

#[test]
fn a_restore_brings_back_a_dropped_database_as_it_was_backed_up() {
    let Some(sql) = sql("a_restore_brings_back_a_dropped_database_as_it_was_backed_up") else {
        return;
    };
    let database = Database::create(&sql, "vssrestore", &sql.first, &sql.first);
    let (id, component, document) = database.back_up();
    assert_eq!(sql.count(&database.name), ROWS + 5);
    database.drop_if_there();

    let restore = Restore::begin(
        &document,
        &[id],
        &RestoreOptions::new().with_timeout(TIMEOUT),
    )
    .unwrap();
    eprintln!("{:?}", restore.writers());
    database.copy_back(&component);
    let writers = restore.finish().unwrap();
    assert!(writers.iter().all(|w| !w.is_failed()), "{writers:?}");

    assert_eq!(sql.state(&database.name), "ONLINE");
    assert_eq!(sql.count(&database.name), ROWS);
}

#[test]
fn aborting_a_restore_before_copying_leaves_the_database_as_it_was() {
    let Some(sql) = sql("aborting_a_restore_before_copying_leaves_the_database_as_it_was") else {
        return;
    };
    let database = Database::create(&sql, "vssabort", &sql.first, &sql.first);
    let (id, _, document) = database.back_up();

    let restore = Restore::begin(
        &document,
        &[id],
        &RestoreOptions::new().with_timeout(TIMEOUT),
    )
    .unwrap();
    eprintln!("during the restore: {}", sql.state(&database.name));
    restore.abort().unwrap();

    assert_eq!(sql.state(&database.name), "ONLINE");
    assert_eq!(sql.count(&database.name), ROWS + 5);
}

#[test]
fn a_backup_that_leaves_out_a_volume_with_the_log_is_refused() {
    let Some(sql) = sql("a_backup_that_leaves_out_a_volume_with_the_log_is_refused") else {
        return;
    };
    let database = Database::create(&sql, "vsspartial", &sql.first, &sql.second);
    let (id, _) = database.component();
    let options = SnapshotOptions::new()
        .with_timeout(TIMEOUT)
        .with_components([id]);
    match SnapshotSet::create([&sql.first], &options) {
        Err(error @ Error::WriterFailed { .. }) => eprintln!("{error}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_component_not_in_the_document_is_not_found() {
    let Some(sql) = sql("a_component_not_in_the_document_is_not_found") else {
        return;
    };
    let database = Database::create(&sql, "vssmissing", &sql.first, &sql.first);
    let (id, _, document) = database.back_up();
    let other = ComponentId::new(id.writer_id, id.logical_path.clone(), "not backed up");
    match Restore::begin(
        &document,
        std::slice::from_ref(&other),
        &RestoreOptions::new(),
    ) {
        Err(Error::ComponentNotFound { component }) => assert_eq!(component, other),
        result => panic!("{result:?}"),
    }
    assert_eq!(sql.state(&database.name), "ONLINE");
}

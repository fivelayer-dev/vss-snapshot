//! Backing up and restoring a SQL Server database through its VSS writer.
//!
//! These run only in the SQL Server workflow on GitHub Actions, which installs SQL Server
//! Express, names its instance in `VSS_SNAPSHOT_TEST_SQL_INSTANCE`, and makes two test
//! volumes, named in `VSS_SNAPSHOT_TEST_VOLUMES`, with a `sql` folder on each that SQL Server
//! may write to. Everywhere else each test prints `SKIPPED` and passes.
//!
//! The tests named `crash_` kill a process partway through, then wait minutes for Windows to
//! notice. They also need `VSS_SNAPSHOT_TEST_CRASH=1`, which only a manual run of that workflow
//! sets.
#![cfg(windows)]

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

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

/// How long a killed process's snapshot may linger. COM can take minutes to notice.
const ORPHAN_LIMIT: Duration = Duration::from_secs(12 * 60);
/// How long to watch a database after a killed restore, in case its writer notices.
const RESTORE_WATCH: Duration = Duration::from_secs(120);

fn crash(test: &str) -> Option<Sql> {
    let sql = sql(test)?;
    if std::env::var("VSS_SNAPSHOT_TEST_CRASH").as_deref() != Ok("1") {
        eprintln!("SKIPPED {test}: only a manual workflow run sets VSS_SNAPSHOT_TEST_CRASH");
        return None;
    }
    Some(sql)
}

/// Not a real test. The crash tests run this binary again with
/// `VSS_SNAPSHOT_TEST_CRASH_CHILD` set, to get a process they can kill while it holds a
/// snapshot or is partway through a restore.
#[test]
fn crash_child_helper() {
    let Ok(mode) = std::env::var("VSS_SNAPSHOT_TEST_CRASH_CHILD") else {
        return;
    };
    assert!(
        std::env::var_os("GITHUB_ACTIONS").is_some(),
        "real snapshots are only taken on GitHub Actions"
    );
    let env = |name: &str| std::env::var(name).unwrap();
    let _held: Box<dyn std::any::Any> = match mode.as_str() {
        "snapshot" => {
            let options = SnapshotOptions::new().with_timeout(TIMEOUT);
            let snapshots =
                SnapshotSet::create([env("VSS_SNAPSHOT_TEST_CRASH_VOLUME")], &options).unwrap();
            println!("READY {}", snapshots.volumes()[0].device_path().display());
            Box::new(snapshots)
        }
        "restore" => {
            let xml = fs::read_to_string(env("VSS_SNAPSHOT_TEST_CRASH_DOCUMENT")).unwrap();
            let id = ComponentId::new(
                env("VSS_SNAPSHOT_TEST_CRASH_WRITER").parse().unwrap(),
                env("VSS_SNAPSHOT_TEST_CRASH_PATH"),
                env("VSS_SNAPSHOT_TEST_CRASH_NAME"),
            );
            let options = RestoreOptions::new().with_timeout(TIMEOUT);
            let restore = Restore::begin(&BackupDocument::from_xml(xml), &[id], &options).unwrap();
            println!("READY begun");
            Box::new(restore)
        }
        other => panic!("unknown mode {other}"),
    };
    std::io::stdout().flush().unwrap();
    loop {
        thread::sleep(Duration::from_secs(60));
    }
}

/// A child process, killed when this is dropped.
struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Starts `crash_child_helper` and waits until it's ready. Returns the process to kill, and
/// what it reported.
fn start_child(mode: &str, envs: &[(&str, &str)]) -> (KillOnDrop, String) {
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["crash_child_helper", "--exact", "--nocapture"])
        .env("VSS_SNAPSHOT_TEST_CRASH_CHILD", mode)
        .envs(envs.iter().copied())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let child = KillOnDrop(child);
    for line in BufReader::new(stdout).lines() {
        let line = line.unwrap();
        if let Some((_, reported)) = line.split_once("READY ") {
            return (child, reported.to_owned());
        }
    }
    panic!("the child process stopped before it was ready");
}

#[test]
fn crash_a_killed_processs_snapshot_is_removed_and_new_ones_still_work() {
    let Some(sql) = crash("crash_a_killed_processs_snapshot_is_removed_and_new_ones_still_work")
    else {
        return;
    };
    let (child, device) = start_child(
        "snapshot",
        &[("VSS_SNAPSHOT_TEST_CRASH_VOLUME", sql.first.as_str())],
    );
    let device = PathBuf::from(device);
    assert!(device.exists(), "the child's snapshot isn't there");

    drop(child);
    let killed = Instant::now();

    let options = SnapshotOptions::new().with_timeout(TIMEOUT);
    let snapshots = loop {
        match SnapshotSet::create([&sql.first], &options) {
            Ok(snapshots) => break snapshots,
            Err(error) if error.is_retryable() && killed.elapsed() < ORPHAN_LIMIT => {
                eprintln!("retrying after {:?}: {error}", killed.elapsed());
                thread::sleep(Duration::from_secs(5));
            }
            Err(error) => panic!("a new snapshot failed after the kill: {error}"),
        }
    };
    eprintln!(
        "MEASURED new snapshot taken {:?} after the kill",
        killed.elapsed()
    );
    assert_ne!(snapshots.volumes()[0].device_path(), device.as_path());
    snapshots.close().unwrap();

    while device.exists() {
        assert!(
            killed.elapsed() < ORPHAN_LIMIT,
            "the snapshot outlived its process by {ORPHAN_LIMIT:?}"
        );
        thread::sleep(Duration::from_secs(2));
    }
    eprintln!(
        "MEASURED killed process's snapshot removed after {:?}",
        killed.elapsed()
    );
}

#[test]
fn crash_a_restore_killed_partway_can_be_run_again() {
    let Some(sql) = crash("crash_a_restore_killed_partway_can_be_run_again") else {
        return;
    };
    let database = Database::create(&sql, "vsskilled", &sql.first, &sql.first);
    let (id, component, document) = database.back_up();
    let stored = database.backup.join("document.xml");
    fs::write(&stored, document.as_xml()).unwrap();

    let writer = id.writer_id.to_string();
    let (child, _) = start_child(
        "restore",
        &[
            ("VSS_SNAPSHOT_TEST_CRASH_DOCUMENT", stored.to_str().unwrap()),
            ("VSS_SNAPSHOT_TEST_CRASH_WRITER", writer.as_str()),
            ("VSS_SNAPSHOT_TEST_CRASH_PATH", id.logical_path.as_str()),
            ("VSS_SNAPSHOT_TEST_CRASH_NAME", id.name.as_str()),
        ],
    );
    assert_eq!(sql.state(&database.name), "RESTORING");

    drop(child);
    let killed = Instant::now();
    while killed.elapsed() < RESTORE_WATCH {
        assert_eq!(
            sql.state(&database.name),
            "RESTORING",
            "changed {:?} after the kill",
            killed.elapsed()
        );
        thread::sleep(Duration::from_secs(5));
    }
    eprintln!(
        "MEASURED still RESTORING {:?} after the kill",
        killed.elapsed()
    );

    let restore = Restore::begin(
        &document,
        std::slice::from_ref(&id),
        &RestoreOptions::new().with_timeout(TIMEOUT),
    )
    .unwrap();
    database.copy_back(&component);
    restore.finish().unwrap();
    assert_eq!(sql.state(&database.name), "ONLINE");
    assert_eq!(sql.count(&database.name), ROWS);
}

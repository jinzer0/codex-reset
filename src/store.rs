//! Durable, at-most-once reservations. This module never performs network I/O.
use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::{self, DirBuilder, OpenOptions};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

const DATABASE: &str = "rust-jobs.sqlite3";
const GRACE_SECONDS: f64 = 5.0;
const OUTCOMES: &[&str] = &["reset", "nothingToReset", "noCredit", "alreadyRedeemed"];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Binding {
    pub account_fingerprint: String,
    pub codex_home: PathBuf,
    pub binary_path: PathBuf,
    pub binary_sha256: String,
    pub binary_version: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Job {
    pub id: String,
    pub idempotency_key: String,
    pub due_at: f64,
    pub grace_seconds: f64,
    pub status: String,
    pub outcome: Option<String>,
    pub error: Option<String>,
    pub created_at: f64,
    pub claimed_at: Option<f64>,
    pub finished_at: Option<f64>,
    pub binding: Binding,
    pub evidence: Option<Value>,
}

#[derive(Clone, Debug)]
pub struct Store {
    path: PathBuf,
}

impl Store {
    pub fn open(directory: &Path) -> Result<Self> {
        ensure!(
            directory.is_absolute(),
            "State directory must be an absolute path"
        );
        if !directory.exists() {
            // Every newly created parent is private too. Existing parents are untouched.
            DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(directory)?;
        }
        let metadata = fs::symlink_metadata(directory)?;
        ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "State directory must not be a symlink"
        );
        ensure!(
            metadata.uid() == current_uid() && metadata.mode() & 0o077 == 0,
            "State directory must belong to you with mode 0700"
        );
        // Resolve system aliases such as macOS /var -> /private/var before
        // using SQLite NOFOLLOW; the requested final directory itself was rejected
        // above if symlinked. The database path still cannot be a symlink.
        let path = fs::canonicalize(directory)?.join(DATABASE);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(_) => (),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(error.into()),
        }
        validate_database(&path)?;
        let store = Self { path };
        store.connect()?.execute_batch(
            "CREATE TABLE IF NOT EXISTS jobs (
                id TEXT PRIMARY KEY,
                idempotency_key TEXT UNIQUE NOT NULL,
                due_at REAL NOT NULL,
                grace_seconds REAL NOT NULL,
                status TEXT NOT NULL,
                outcome TEXT,
                error TEXT,
                created_at REAL NOT NULL,
                claimed_at REAL,
                finished_at REAL,
                binding TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS job_evidence (
                job_id TEXT PRIMARY KEY,
                document TEXT NOT NULL
            );",
        )?;
        Ok(store)
    }

    fn connect(&self) -> Result<Connection> {
        validate_database(&self.path)?;
        let connection = Connection::open_with_flags(
            &self.path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        connection.busy_timeout(Duration::from_secs(15))?;
        connection.execute_batch("PRAGMA trusted_schema=OFF; PRAGMA synchronous=FULL;")?;
        Ok(connection)
    }

    pub fn create(&self, due_at: f64, binding: Binding) -> Result<Job> {
        self.create_at(due_at, binding, now())
    }

    fn create_at(&self, due_at: f64, binding: Binding, created_at: f64) -> Result<Job> {
        ensure!(
            due_at.is_finite() && created_at.is_finite() && due_at > created_at,
            "Schedule must be in the future"
        );
        let id = Uuid::new_v4().to_string();
        self.connect()?.execute(
            "INSERT INTO jobs VALUES (?1, ?2, ?3, ?4, 'pending', NULL, NULL, ?5, NULL, NULL, ?6)",
            params![
                id,
                Uuid::new_v4().to_string(),
                due_at,
                GRACE_SECONDS,
                created_at,
                serde_json::to_string(&binding)?
            ],
        )?;
        self.show(&id)
    }

    pub fn list(&self) -> Result<Vec<Job>> {
        let connection = self.connect()?;
        let mut statement = connection.prepare(
            "SELECT jobs.*, job_evidence.document FROM jobs LEFT JOIN job_evidence ON jobs.id=job_evidence.job_id ORDER BY due_at, id",
        )?;
        let rows = statement.query_map([], row_job)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn show(&self, id: &str) -> Result<Job> {
        self.connect()?
            .query_row("SELECT jobs.*, job_evidence.document FROM jobs LEFT JOIN job_evidence ON jobs.id=job_evidence.job_id WHERE jobs.id=?1", [id], row_job)
            .optional()?
            .context("Reservation was not found")
    }

    pub fn resolve_id(&self, prefix: &str) -> Result<String> {
        ensure!(
            prefix.len() >= 8,
            "Use a unique job ID of at least 8 characters"
        );
        let matches: Vec<_> = self
            .list()?
            .into_iter()
            .filter(|job| job.id.starts_with(prefix))
            .collect();
        ensure!(
            matches.len() == 1,
            "Use a unique job ID; see codex-reset status"
        );
        Ok(matches[0].id.clone())
    }

    /// The conditional update races safely against claim(): exactly one can win.
    pub fn cancel(&self, id: &str) -> Result<Job> {
        self.connect()?.execute(
            "UPDATE jobs SET status='cancelled' WHERE id=?1 AND status='pending'",
            [id],
        )?;
        self.show(id)
    }

    /// Return a committed claim only while dispatch is still in its five-second window.
    /// A claimed job is never automatically returned to pending, including after a crash.
    pub fn claim(&self, id: &str) -> Result<Option<Job>> {
        self.claim_with_clock(id, now)
    }

    fn claim_with_clock(&self, id: &str, mut clock: impl FnMut() -> f64) -> Result<Option<Job>> {
        let mut connection = self.connect()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut job = transaction
            .query_row("SELECT jobs.*, job_evidence.document FROM jobs LEFT JOIN job_evidence ON jobs.id=job_evidence.job_id WHERE jobs.id=?1", [id], row_job)
            .optional()?
            .context("Reservation was not found")?;
        // Taking the lock can block: read wall time only after acquiring it.
        let checked_at = clock();
        ensure!(checked_at.is_finite(), "Clock must be finite");
        if job.status != "pending" || checked_at < job.due_at {
            return Ok(None);
        }
        if checked_at > job.due_at + job.grace_seconds {
            transaction.execute(
                "UPDATE jobs SET status='expired', finished_at=?1 WHERE id=?2",
                params![checked_at, id],
            )?;
            transaction.commit()?;
            return Ok(None);
        }
        transaction.execute(
            "UPDATE jobs SET status='claimed', claimed_at=?1 WHERE id=?2",
            params![checked_at, id],
        )?;
        transaction.commit()?;
        job.status = "claimed".to_owned();
        job.claimed_at = Some(checked_at);
        // No external action may run under the claim transaction. Also account for a
        // slow commit so returning Some never authorizes an already-expired dispatch.
        let dispatch_at = clock();
        ensure!(dispatch_at.is_finite(), "Clock must be finite");
        if dispatch_at < job.due_at || dispatch_at > job.due_at + job.grace_seconds {
            self.connect()?.execute(
                "UPDATE jobs SET status='expired', finished_at=?1 WHERE id=?2 AND status='claimed'",
                params![dispatch_at, id],
            )?;
            return Ok(None);
        }
        Ok(Some(job))
    }

    /// Persist the result once. Unknown responses and errors are never retried.
    pub fn finish(&self, id: &str, outcome: Option<&str>, error: Option<&str>) -> Result<Job> {
        let success = error.is_none() && outcome.is_some_and(|value| OUTCOMES.contains(&value));
        let (status, saved_outcome, saved_error) = if success {
            ("completed", outcome, None)
        } else {
            (
                "unknown",
                None,
                Some(error.unwrap_or("UnrecognizedOutcome")),
            )
        };
        self.connect()?.execute(
            "UPDATE jobs SET status=?1, outcome=?2, error=?3, finished_at=?4 WHERE id=?5 AND status='claimed'",
            params![status, saved_outcome, saved_error, now(), id],
        )?;
        self.show(id)
    }

    pub fn fail_preflight(&self, id: &str, error: &str) -> Result<()> {
        self.connect()?.execute(
            "UPDATE jobs SET status='failed', error=?1, finished_at=?2 WHERE id=?3 AND status='pending'",
            params![error, now(), id],
        )?;
        Ok(())
    }

    /// Save evidence independently of the terminal result, including during readback.
    pub fn record_evidence(&self, id: &str, evidence: &Value) -> Result<()> {
        let changed = self.connect()?.execute(
            "INSERT INTO job_evidence (job_id, document)
             SELECT id, ?2 FROM jobs WHERE id=?1 AND status IN ('claimed', 'completed', 'unknown')
             ON CONFLICT(job_id) DO UPDATE SET document=excluded.document",
            params![id, serde_json::to_string(evidence)?],
        )?;
        ensure!(changed == 1, "Evidence requires a claimed or finished job");
        Ok(())
    }
}

fn current_uid() -> u32 {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

fn validate_database(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "Database must be a regular file, not a symlink"
    );
    ensure!(
        metadata.uid() == current_uid() && metadata.mode() & 0o077 == 0,
        "Database must belong to you with mode 0600"
    );
    ensure!(
        metadata.nlink() == 1,
        "Database must not have additional hard links"
    );
    Ok(())
}

fn row_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<Job> {
    let binding: String = row.get(10)?;
    let binding = serde_json::from_str(&binding).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(10, rusqlite::types::Type::Text, Box::new(error))
    })?;
    let evidence: Option<String> = row.get(11)?;
    let evidence = evidence
        .map(|text| serde_json::from_str(&text))
        .transpose()
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                11,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?;
    Ok(Job {
        id: row.get(0)?,
        idempotency_key: row.get(1)?,
        due_at: row.get(2)?,
        grace_seconds: row.get(3)?,
        status: row.get(4)?,
        outcome: row.get(5)?,
        error: row.get(6)?,
        created_at: row.get(7)?,
        claimed_at: row.get(8)?,
        finished_at: row.get(9)?,
        binding,
        evidence,
    })
}

pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

fn timestamp(value: DateTime<impl TimeZone>) -> f64 {
    value.timestamp() as f64 + f64::from(value.timestamp_subsec_nanos()) / 1_000_000_000.0
}

fn local_timestamp(value: NaiveDateTime, zone: Tz) -> Result<f64> {
    zone.from_local_datetime(&value)
        .single()
        .map(timestamp)
        .context("Ambiguous or nonexistent local time; specify an explicit UTC offset")
}

/// Limited explicit syntax, with no fuzzy natural-language interpretation.
pub fn parse_time(value: &str, timezone: &str, current: f64) -> Result<f64> {
    let zone: Tz = timezone
        .parse()
        .context("Use a valid IANA timezone, such as Asia/Seoul")?;
    ensure!(current.is_finite(), "Current time must be finite");
    let local_now = DateTime::<Utc>::from_timestamp_millis((current * 1000.0).floor() as i64)
        .context("Current time is outside the supported date range")?
        .with_timezone(&zone);
    let value = value.trim();
    if let Ok(date) = DateTime::parse_from_rfc3339(value) {
        return Ok(timestamp(date));
    }
    for format in [
        "%Y-%m-%dT%H:%M%:z",
        "%Y-%m-%d %H:%M%:z",
        "%Y-%m-%d %H:%M:%S%.f%:z",
    ] {
        if let Ok(date) = DateTime::parse_from_str(value, format) {
            return Ok(timestamp(date));
        }
    }
    let parts: Vec<_> = value.split_whitespace().collect();
    let (day, clock) = match parts.as_slice() {
        [clock] if clock.len() == 5 && clock.as_bytes().get(2) == Some(&b':') => (None, *clock),
        [day @ ("today" | "tomorrow" | "오늘" | "내일"), clock] => (Some(*day), *clock),
        _ => (None, ""),
    };
    if !clock.is_empty() {
        ensure!(clock.len() == 5, "Use HH:MM for the time");
        let clock = NaiveTime::parse_from_str(clock, "%H:%M").context("Use a valid HH:MM time")?;
        let mut date = local_now.date_naive();
        if matches!(day, Some("tomorrow" | "내일")) {
            date = date
                .succ_opt()
                .context("Date is outside the supported range")?;
        }
        let mut due = local_timestamp(date.and_time(clock), zone)?;
        if day.is_none() && due <= current {
            date = date
                .succ_opt()
                .context("Date is outside the supported range")?;
            due = local_timestamp(date.and_time(clock), zone)?;
        }
        return Ok(due);
    }
    for format in [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(date) = NaiveDateTime::parse_from_str(value, format) {
            return local_timestamp(date, zone);
        }
    }
    if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        return local_timestamp(date.and_time(NaiveTime::MIN), zone);
    }
    bail!("Use HH:MM, tomorrow HH:MM, or an ISO date and time")
}

pub fn local_zone() -> String {
    let mut candidates = Vec::new();
    if let Ok(zone) = std::env::var("TZ") {
        candidates.push(zone.trim_start_matches(':').to_owned());
    }
    if let Ok(zone) = fs::read_to_string("/etc/timezone") {
        candidates.push(zone.trim().to_owned());
    }
    if let Ok(path) = fs::canonicalize("/etc/localtime")
        && let Some((_, zone)) = path.to_string_lossy().split_once("/zoneinfo/")
    {
        candidates.push(zone.to_owned());
    }
    candidates
        .into_iter()
        .find(|zone| zone.parse::<Tz>().is_ok())
        .unwrap_or_else(|| "UTC".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::sync::{Arc, Barrier};
    use std::thread;

    fn binding() -> Binding {
        Binding {
            account_fingerprint: "fake-account".into(),
            codex_home: PathBuf::from("/fake/home"),
            binary_path: PathBuf::from("/fake/codex"),
            binary_sha256: "fake-digest".into(),
            binary_version: "0.165.0".into(),
        }
    }

    fn fixture() -> (tempfile::TempDir, Store) {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("state")).unwrap();
        (root, store)
    }

    #[test]
    fn evidence_survives_terminal_result_and_database_reopen() {
        let (root, store) = fixture();
        let job = store.create_at(100.0, binding(), 90.0).unwrap();
        let before = serde_json::json!({"phase":"consume","before":{"usedPercent":80}});
        assert!(store.record_evidence(&job.id, &before).is_err());
        store.claim_with_clock(&job.id, || 101.0).unwrap().unwrap();
        store.record_evidence(&job.id, &before).unwrap();
        assert_eq!(store.show(&job.id).unwrap().evidence, Some(before));
        store.finish(&job.id, Some("reset"), None).unwrap();
        let verified =
            serde_json::json!({"server_outcome":"reset","verification":{"status":"confirmed"}});
        store.record_evidence(&job.id, &verified).unwrap();
        let reopened = Store::open(&root.path().join("state")).unwrap();
        assert_eq!(reopened.show(&job.id).unwrap().evidence, Some(verified));
        assert_eq!(
            reopened.list().unwrap()[0].outcome.as_deref(),
            Some("reset")
        );
        assert!(
            reopened
                .claim_with_clock(&job.id, || 102.0)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn opening_legacy_database_preserves_jobs_and_adds_optional_evidence() {
        let (root, store) = fixture();
        let job = store.create_at(100.0, binding(), 90.0).unwrap();
        store
            .connect()
            .unwrap()
            .execute_batch("DROP TABLE job_evidence")
            .unwrap();
        let reopened = Store::open(&root.path().join("state")).unwrap();
        assert_eq!(reopened.show(&job.id).unwrap(), job);
        assert_eq!(reopened.list().unwrap(), vec![job]);
    }

    #[test]
    fn persists_identity_and_claim_across_reopen() {
        let (root, store) = fixture();
        let job = store.create_at(100.0, binding(), 90.0).unwrap();
        assert!(store.claim_with_clock(&job.id, || 99.0).unwrap().is_none());
        assert_eq!(
            store
                .claim_with_clock(&job.id, || 101.0)
                .unwrap()
                .unwrap()
                .status,
            "claimed"
        );
        let reopened = Store::open(&root.path().join("state")).unwrap();
        assert!(
            reopened
                .claim_with_clock(&job.id, || 102.0)
                .unwrap()
                .is_none()
        );
        assert_eq!(reopened.show(&job.id).unwrap().binding, binding());
        assert_eq!(
            reopened.show(&job.id).unwrap().idempotency_key,
            job.idempotency_key
        );
        assert_eq!(
            reopened
                .finish(&job.id, Some("reset"), None)
                .unwrap()
                .status,
            "completed"
        );
        assert_eq!(
            reopened
                .finish(&job.id, None, Some("LateError"))
                .unwrap()
                .outcome
                .as_deref(),
            Some("reset")
        );
    }

    #[test]
    fn concurrent_workers_receive_only_one_claim() {
        let (_root, store) = fixture();
        let job = store.create_at(100.0, binding(), 90.0).unwrap();
        let barrier = Arc::new(Barrier::new(8));
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let store = store.clone();
                let barrier = barrier.clone();
                let id = job.id.clone();
                thread::spawn(move || {
                    barrier.wait();
                    store.claim_with_clock(&id, || 101.0).unwrap().is_some()
                })
            })
            .collect();
        assert_eq!(
            workers
                .into_iter()
                .map(|worker| worker.join().unwrap())
                .filter(|claimed| *claimed)
                .count(),
            1
        );
    }

    #[test]
    fn cancellation_wins_or_preserves_claim() {
        let (_root, store) = fixture();
        let job = store.create_at(100.0, binding(), 90.0).unwrap();
        assert_eq!(store.cancel(&job.id).unwrap().status, "cancelled");
        assert!(store.claim_with_clock(&job.id, || 101.0).unwrap().is_none());
        let other = store.create_at(100.0, binding(), 90.0).unwrap();
        store.claim_with_clock(&other.id, || 101.0).unwrap();
        assert_eq!(store.cancel(&other.id).unwrap().status, "claimed");
    }

    #[test]
    fn cancellation_race_never_releases_a_claim() {
        let (_root, store) = fixture();
        let job = store.create_at(100.0, binding(), 90.0).unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let claimant = {
            let store = store.clone();
            let id = job.id.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                store.claim_with_clock(&id, || 101.0).unwrap().is_some()
            })
        };
        barrier.wait();
        let cancelled = store.cancel(&job.id).unwrap();
        let claimed = claimant.join().unwrap();
        assert_eq!(
            store.show(&job.id).unwrap().status,
            if claimed { "claimed" } else { "cancelled" }
        );
        assert_eq!(
            cancelled.status,
            if claimed { "claimed" } else { "cancelled" }
        );
    }

    #[test]
    fn clock_sampling_waits_for_the_sqlite_writer_lock() {
        let (_root, store) = fixture();
        let job = store.create_at(100.0, binding(), 90.0).unwrap();
        let mut connection = store.connect().unwrap();
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (clock_tx, clock_rx) = std::sync::mpsc::channel();
        let worker = {
            let store = store.clone();
            let id = job.id.clone();
            thread::spawn(move || {
                ready_tx.send(()).unwrap();
                store
                    .claim_with_clock(&id, || {
                        clock_tx.send(()).unwrap();
                        106.0
                    })
                    .unwrap()
            })
        };
        ready_rx.recv().unwrap();
        assert!(clock_rx.recv_timeout(Duration::from_millis(50)).is_err());
        transaction.commit().unwrap();
        assert!(worker.join().unwrap().is_none());
        assert_eq!(store.show(&job.id).unwrap().status, "expired");
    }

    #[test]
    fn late_jobs_expire_without_a_claim() {
        let (_root, store) = fixture();
        let job = store.create_at(100.0, binding(), 90.0).unwrap();
        assert!(
            store
                .claim_with_clock(&job.id, || 105.001)
                .unwrap()
                .is_none()
        );
        let expired = store.show(&job.id).unwrap();
        assert_eq!(expired.status, "expired");
        assert_eq!(expired.claimed_at, None);
    }

    #[test]
    fn clock_is_checked_again_after_commit() {
        let (_root, store) = fixture();
        let job = store.create_at(100.0, binding(), 90.0).unwrap();
        let mut moments = [101.0, 106.0].into_iter();
        assert!(
            store
                .claim_with_clock(&job.id, || moments.next().unwrap())
                .unwrap()
                .is_none()
        );
        assert_eq!(store.show(&job.id).unwrap().status, "expired");
    }

    #[test]
    fn failed_or_unknown_jobs_are_not_retried() {
        let (_root, store) = fixture();
        let job = store.create_at(100.0, binding(), 90.0).unwrap();
        store.fail_preflight(&job.id, "BinaryChanged").unwrap();
        assert_eq!(store.show(&job.id).unwrap().status, "failed");
        assert!(store.claim_with_clock(&job.id, || 101.0).unwrap().is_none());
        let other = store.create_at(100.0, binding(), 90.0).unwrap();
        store.claim_with_clock(&other.id, || 101.0).unwrap();
        store.fail_preflight(&other.id, "MustNotOverwrite").unwrap();
        assert_eq!(store.show(&other.id).unwrap().status, "claimed");
        let result = store.finish(&other.id, Some("unexpected"), None).unwrap();
        assert_eq!(result.status, "unknown");
        assert_eq!(result.error.as_deref(), Some("UnrecognizedOutcome"));
        assert!(
            store
                .claim_with_clock(&other.id, || 102.0)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn rejects_nonfinite_or_past_reservations_and_bad_prefixes() {
        let (_root, store) = fixture();
        for due in [f64::NAN, f64::INFINITY, 89.0, 90.0] {
            assert!(store.create_at(due, binding(), 90.0).is_err());
        }
        let job = store.create_at(100.0, binding(), 90.0).unwrap();
        assert_eq!(store.resolve_id(&job.id[..8]).unwrap(), job.id);
        assert!(store.resolve_id(&job.id[..7]).is_err());
        assert!(store.resolve_id("00000000").is_err());
    }

    #[test]
    fn private_permissions_and_symlink_rejection() {
        let (root, _store) = fixture();
        let state = root.path().join("state");
        assert_eq!(fs::metadata(&state).unwrap().mode() & 0o777, 0o700);
        assert_eq!(
            fs::metadata(state.join(DATABASE)).unwrap().mode() & 0o777,
            0o600
        );
        let link = root.path().join("link");
        symlink(&state, &link).unwrap();
        assert!(Store::open(&link).is_err());
        fs::set_permissions(&state, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(Store::open(&state).is_err());
    }

    #[test]
    fn database_symlinks_and_public_files_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        let state = root.path().join("state");
        fs::create_dir(&state).unwrap();
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
        let target = root.path().join("target");
        fs::write(&target, "sentinel").unwrap();
        symlink(&target, state.join(DATABASE)).unwrap();
        assert!(Store::open(&state).is_err());
        assert_eq!(fs::read_to_string(target).unwrap(), "sentinel");
        fs::remove_file(state.join(DATABASE)).unwrap();
        fs::write(state.join(DATABASE), "").unwrap();
        fs::set_permissions(state.join(DATABASE), fs::Permissions::from_mode(0o644)).unwrap();
        assert!(Store::open(&state).is_err());
    }

    #[test]
    fn simple_clock_and_korean_tomorrow_are_explicit() {
        let current = parse_time("2030-01-01T05:00:00+09:00", "Asia/Seoul", 0.0).unwrap();
        let next = parse_time("2030-01-02T04:00:00+09:00", "Asia/Seoul", 0.0).unwrap();
        for value in ["04:00", "tomorrow 04:00", "내일 04:00"] {
            assert_eq!(parse_time(value, "Asia/Seoul", current).unwrap(), next);
        }
        assert_eq!(
            parse_time("오늘 04:00", "Asia/Seoul", current).unwrap(),
            next - 86400.0
        );
        assert_eq!(
            parse_time("2030-01-02 04:00", "Asia/Seoul", current).unwrap(),
            next
        );
        assert_eq!(
            parse_time("2030-01-02T04:00+09:00", "UTC", current).unwrap(),
            next
        );
    }

    #[test]
    fn rejects_ambiguous_and_nonexistent_dst_but_accepts_offset() {
        for value in ["2030-11-03 01:30", "2030-03-10 02:30"] {
            assert!(parse_time(value, "America/New_York", 0.0).is_err());
        }
        assert!(parse_time("2030-11-03T01:30:00-04:00", "America/New_York", 0.0).is_ok());
        assert!(parse_time("2030-11-03T01:30:00-05:00", "America/New_York", 0.0).is_ok());
        assert!(parse_time("25:00", "UTC", 0.0).is_err());
        assert!(parse_time("04:00", "invalid-zone", 0.0).is_err());
    }
}

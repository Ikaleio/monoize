use sea_orm::{
    ConnectOptions, Database, DatabaseConnection, DatabaseTransaction, DbBackend, DbErr, Statement,
    TransactionTrait, Value,
};
use sqlx::sqlite::{SqliteJournalMode, SqliteSynchronous};
use std::ops::Deref;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

pub struct WriteGuard<'a> {
    conn: &'a DatabaseConnection,
    _guard: Option<tokio::sync::MutexGuard<'a, ()>>,
}

impl Deref for WriteGuard<'_> {
    type Target = DatabaseConnection;
    fn deref(&self) -> &Self::Target {
        self.conn
    }
}

pub struct WriteTransaction {
    txn: Option<DatabaseTransaction>,
    #[allow(dead_code)]
    _guard: Option<Box<tokio::sync::OwnedMutexGuard<()>>>,
}

impl WriteTransaction {
    pub async fn commit(mut self) -> Result<(), DbErr> {
        if let Some(txn) = self.txn.take() {
            txn.commit().await?;
        }
        Ok(())
    }

    pub async fn rollback(mut self) -> Result<(), DbErr> {
        if let Some(txn) = self.txn.take() {
            txn.rollback().await?;
        }
        Ok(())
    }
}

impl Deref for WriteTransaction {
    type Target = DatabaseTransaction;
    fn deref(&self) -> &Self::Target {
        self.txn.as_ref().expect("transaction already consumed")
    }
}

/// Wraps a pair of Sea ORM connections: one for writes (single-connection for SQLite,
/// standard pool for PostgreSQL) and one for reads (10-connection pool for SQLite,
/// shared with write pool for PostgreSQL).
///
/// For SQLite, all write access is serialized through a tokio Mutex to prevent
/// concurrent write failures and billing bypass via race conditions.
#[derive(Debug, Clone)]
pub struct DbPool {
    read: DatabaseConnection,
    write_conn: DatabaseConnection,
    write_lock: Arc<Mutex<()>>,
    backend: DbBackend,
}

impl DbPool {
    /// Create a new DbPool from a database DSN.
    ///
    /// For SQLite DSNs (starting with "sqlite://"):
    ///   - Creates a write pool with max 1 connection (single-writer)
    ///   - Creates a read pool with max 10 connections
    ///   - Applies WAL mode and connection-local PRAGMAs, including a 15s busy timeout
    ///
    /// For PostgreSQL DSNs (starting with "postgres://" or "postgresql://"):
    ///   - Creates a single connection pool used for both reads and writes
    ///   - Default pool settings from Sea ORM
    pub async fn connect(dsn: &str) -> Result<Self, DbErr> {
        let dsn = dsn.trim();
        if dsn.starts_with("sqlite://") || dsn.starts_with("sqlite::memory:") {
            Self::connect_sqlite(dsn).await
        } else if dsn.starts_with("postgres://") || dsn.starts_with("postgresql://") {
            Self::connect_postgres(dsn).await
        } else {
            Err(DbErr::Custom(format!(
                "unsupported database DSN scheme: {dsn}"
            )))
        }
    }

    async fn connect_sqlite(dsn: &str) -> Result<Self, DbErr> {
        ensure_sqlite_file(dsn).map_err(DbErr::Custom)?;

        if is_sqlite_memory_dsn(dsn) {
            let opts = Self::sqlite_connect_options(dsn, 1);
            let conn = Database::connect(opts).await?;
            return Ok(Self {
                read: conn.clone(),
                write_conn: conn,
                write_lock: Arc::new(Mutex::new(())),
                backend: DbBackend::Sqlite,
            });
        }

        let base_dsn = if dsn.contains('?') {
            dsn.to_string()
        } else {
            format!("{dsn}?mode=rwc")
        };

        let write_opts = Self::sqlite_connect_options(&base_dsn, 1);
        let read_opts = Self::sqlite_connect_options(&base_dsn, 10);

        let write = Database::connect(write_opts).await?;
        let read = Database::connect(read_opts).await?;

        Ok(Self {
            read,
            write_conn: write,
            write_lock: Arc::new(Mutex::new(())),
            backend: DbBackend::Sqlite,
        })
    }

    fn sqlite_connect_options(dsn: &str, max_connections: u32) -> ConnectOptions {
        let mut opts = ConnectOptions::new(dsn);
        opts.max_connections(max_connections)
            .acquire_timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .sqlx_logging(false);
        opts.map_sqlx_sqlite_opts(|opts| {
            opts.journal_mode(SqliteJournalMode::Wal)
                .synchronous(SqliteSynchronous::Normal)
                .busy_timeout(Duration::from_secs(15))
                .foreign_keys(true)
                .pragma("cache_size", "-65536")
                .pragma("mmap_size", "268435456")
        });
        opts
    }

    async fn connect_postgres(dsn: &str) -> Result<Self, DbErr> {
        let opts = ConnectOptions::new(dsn)
            .max_connections(20)
            .acquire_timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .sqlx_logging(false)
            .to_owned();

        let conn = Database::connect(opts).await?;

        Ok(Self {
            read: conn.clone(),
            write_conn: conn,
            write_lock: Arc::new(Mutex::new(())),
            backend: DbBackend::Postgres,
        })
    }

    /// Get the read connection (for SELECT queries).
    pub fn read(&self) -> &DatabaseConnection {
        &self.read
    }

    /// Acquire the write connection. For SQLite, this serializes all writes
    /// through a tokio Mutex to prevent concurrent write failures.
    /// For PostgreSQL, the returned guard holds no lock (no-op).
    pub async fn write(&self) -> WriteGuard<'_> {
        if self.backend == DbBackend::Sqlite {
            let guard = self.write_lock.lock().await;
            WriteGuard {
                conn: &self.write_conn,
                _guard: Some(guard),
            }
        } else {
            WriteGuard {
                conn: &self.write_conn,
                _guard: None,
            }
        }
    }

    /// Get the database backend type.
    pub fn backend(&self) -> DbBackend {
        self.backend
    }

    /// Check if this is a SQLite backend.
    pub fn is_sqlite(&self) -> bool {
        self.backend == DbBackend::Sqlite
    }

    /// Check if this is a PostgreSQL backend.
    pub fn is_postgres(&self) -> bool {
        self.backend == DbBackend::Postgres
    }

    /// Acquire write connection and begin an explicit transaction.
    pub async fn begin_write(&self) -> Result<WriteTransaction, DbErr> {
        let guard = if self.backend == DbBackend::Sqlite {
            Some(Box::new(self.write_lock.clone().lock_owned().await))
        } else {
            None
        };
        let txn = self.write_conn.begin().await?;
        Ok(WriteTransaction {
            txn: Some(txn),
            _guard: guard,
        })
    }

    /// Mark both pools closed and wait for every physical connection to close.
    pub async fn close(&self) -> Result<(), DbErr> {
        self.read.close_by_ref().await?;
        self.write_conn.close_by_ref().await
    }

    /// Create a Statement with automatic placeholder conversion.
    /// Write SQL with $1, $2, ... placeholders.
    /// For SQLite, $N placeholders are auto-converted to numbered ?N placeholders.
    pub fn stmt(&self, sql: &str, values: Vec<Value>) -> Statement {
        if self.backend == DbBackend::Sqlite {
            let mut result = String::with_capacity(sql.len());
            let mut chars = sql.chars().peekable();
            while let Some(ch) = chars.next() {
                if ch == '$' && chars.peek().is_some_and(|c| c.is_ascii_digit()) {
                    result.push('?');
                    while chars.peek().is_some_and(|c| c.is_ascii_digit()) {
                        result.push(chars.next().expect("peeked placeholder digit"));
                    }
                } else {
                    result.push(ch);
                }
            }
            Statement::from_sql_and_values(DbBackend::Sqlite, result, values)
        } else {
            Statement::from_sql_and_values(self.backend, sql, values)
        }
    }
}

fn is_sqlite_memory_dsn(dsn: &str) -> bool {
    let dsn = dsn.trim();
    dsn.starts_with("sqlite::memory:") || dsn.contains(":memory:") || dsn.contains("mode=memory")
}

fn ensure_sqlite_file(dsn: &str) -> Result<(), String> {
    let dsn = dsn.trim();
    if !dsn.starts_with("sqlite://") {
        return Ok(());
    }
    if dsn.contains(":memory:") || dsn.contains("mode=memory") {
        return Ok(());
    }
    let path_part = dsn.trim_start_matches("sqlite://");
    let path_part = path_part.split('?').next().unwrap_or("");
    if path_part.is_empty() {
        return Ok(());
    }
    let path = std::path::PathBuf::from(path_part);
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("sqlite_dir_create_failed: {err}"))?;
        }
    }
    if !path.exists() {
        std::fs::File::create(&path).map_err(|err| format!("sqlite_file_create_failed: {err}"))?;
    }
    Ok(())
}

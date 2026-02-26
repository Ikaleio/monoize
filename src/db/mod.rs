use sea_orm::{ConnectOptions, Database, DatabaseConnection, DbBackend, DbErr, Statement, Value};
use std::ops::Deref;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

/// Serializes write access for SQLite backends. For PostgreSQL the guard is a
/// no-op — PostgreSQL handles write concurrency natively via MVCC.
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
    ///   - Enables WAL journal mode and 5s busy timeout
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
            let opts = ConnectOptions::new(dsn)
                .max_connections(1)
                .acquire_timeout(Duration::from_secs(10))
                .connect_timeout(Duration::from_secs(5))
                .sqlx_logging(false)
                .to_owned();
            let conn = Database::connect(opts).await?;
            Self::sqlite_pragmas(&conn).await?;
            return Ok(Self {
                read: conn.clone(),
                write_conn: conn,
                write_lock: Arc::new(Mutex::new(())),
                backend: DbBackend::Sqlite,
            });
        }

        // SQLite: append WAL + busy_timeout query params if not present
        let base_dsn = if dsn.contains('?') || is_sqlite_memory_dsn(dsn) {
            dsn.to_string()
        } else {
            format!("{dsn}?mode=rwc")
        };

        let write_opts = ConnectOptions::new(&base_dsn)
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .sqlx_logging(false)
            .to_owned();

        let read_opts = ConnectOptions::new(&base_dsn)
            .max_connections(10)
            .acquire_timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .sqlx_logging(false)
            .to_owned();

        let write = Database::connect(write_opts).await?;
        let read = Database::connect(read_opts).await?;

        // Enable WAL mode and busy timeout via PRAGMA on both pools
        Self::sqlite_pragmas(&write).await?;
        Self::sqlite_pragmas(&read).await?;

        Ok(Self {
            read,
            write_conn: write,
            write_lock: Arc::new(Mutex::new(())),
            backend: DbBackend::Sqlite,
        })
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

    async fn sqlite_pragmas(conn: &DatabaseConnection) -> Result<(), DbErr> {
        use sea_orm::ConnectionTrait;
        conn.execute_unprepared("PRAGMA journal_mode=WAL").await?;
        conn.execute_unprepared("PRAGMA busy_timeout=15000").await?;
        conn.execute_unprepared("PRAGMA foreign_keys=ON").await?;
        conn.execute_unprepared("PRAGMA synchronous=NORMAL").await?;
        conn.execute_unprepared("PRAGMA cache_size=-65536").await?;
        conn.execute_unprepared("PRAGMA mmap_size=268435456").await?;
        Ok(())
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

    /// Create a Statement with automatic placeholder conversion.
    /// Write SQL with $1, $2, ... placeholders.
    /// For SQLite, $N placeholders are auto-converted to ?.
    pub fn stmt(&self, sql: &str, values: Vec<Value>) -> Statement {
        if self.backend == DbBackend::Sqlite {
            let mut result = String::with_capacity(sql.len());
            let mut chars = sql.chars().peekable();
            while let Some(ch) = chars.next() {
                if ch == '$' && chars.peek().is_some_and(|c| c.is_ascii_digit()) {
                    while chars.peek().is_some_and(|c| c.is_ascii_digit()) {
                        chars.next();
                    }
                    result.push('?');
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
        std::fs::File::create(&path)
            .map_err(|err| format!("sqlite_file_create_failed: {err}"))?;
    }
    Ok(())
}

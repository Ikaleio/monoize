use tempfile::TempDir;

fn test_runtime(database_dsn: String) -> monoize::app::RuntimeConfig {
    monoize::app::RuntimeConfig {
        listen: "127.0.0.1:0".to_string(),
        metrics_path: "/metrics".to_string(),
        unknown_fields: monoize::config::UnknownFieldPolicy::Preserve,
        database_dsn,
    }
}

#[tokio::test]
async fn sqlite_file_created_for_runtime_dsn() {
    let temp_dir = TempDir::new().expect("temp dir");
    let db_path = temp_dir.path().join("data").join("monoize.db");
    assert!(!db_path.exists());

    let runtime = test_runtime(format!("sqlite://{}", db_path.display()));
    let _state = monoize::app::load_state_with_runtime(runtime)
        .await
        .expect("load state");

    assert!(db_path.exists());
}

#[tokio::test]
async fn sqlite_memory_dsn_starts_without_files() {
    let runtime = test_runtime("sqlite::memory:".to_string());
    let _state = monoize::app::load_state_with_runtime(runtime)
        .await
        .expect("load state");
}

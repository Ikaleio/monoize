# Test Harness Specification

## 0. Scope

TH0.1. This specification defines resource cleanup for Rust integration-test fixtures.

TH0.2. This specification does not change server runtime behavior.

## 1. API fixture lifecycle

TH1.1. Each API test fixture that loads `AppState` with a temporary SQLite database MUST
own that database until the fixture is dropped.

TH1.2. When the fixture is dropped, it MUST set `AppState.background_shutdown` to `true`
before it closes the database pools.

TH1.3. Fixture cleanup MUST transfer the database pool and temporary directory to a cleanup
thread. The cleanup thread MUST call `DbPool::close()` before it drops the temporary directory.

TH1.4. Fixture cleanup MUST use a cleanup runtime that remains available while the test's
Tokio runtime is being destroyed. `Drop` MUST NOT wait for the pool to close because an unfinished
task on the test runtime can still hold a connection until `Drop` returns.

TH1.5. The cleanup thread MUST record a runtime-build or database-close error in process-global
test state. The next fixture setup MUST fail with that recorded error.

TH1.6. Failure to start the cleanup thread MUST fail a non-panicking test. It MUST NOT start a
second panic while the test thread is already panicking.

## 2. Parallel execution

TH2.1. `cargo test --test api` MUST pass with the Rust test runner's default thread count when
the process file-descriptor soft limit is 1024 or greater.

TH2.2. The API test target MUST NOT require `RUST_TEST_THREADS=1`, `--test-threads=1`, or
another serial-runner setting.

TH2.3. The test process MUST NOT delete a temporary SQLite directory before its database pools
close. Completed fixture cleanup MUST leave no open descriptor for the database, WAL, or
shared-memory file.

//! Closed-loop test for the DbPool connection pool (Task #18).
//!
//! Verifies:
//! - Pool hands out multiple connections in parallel
//! - Two concurrent workers don't serialise on a single mutex (a 1-conn
//!   pool would block; an N-conn pool runs them in parallel)
//! - Connections returned to pool after use (no leaks across N tasks)

use magiclaw::infrastructure::db::{init_db, init_db_pool, DbPool};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[test]
fn single_connection_pool_preserves_old_behaviour() {
    // DbPool::new(init_db(...)) keeps the legacy single-connection API
    // used by most existing tests.
    let pool = DbPool::new(init_db(":memory:").unwrap());
    assert_eq!(pool.capacity(), 1);
    pool.execute(|_c| Ok(())).unwrap();
}

#[test]
fn multi_connection_pool_capacity() {
    let pool = init_db_pool(":memory:", 4).unwrap();
    assert_eq!(pool.capacity(), 4);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn parallel_workers_actually_run_in_parallel() {
    // 4-connection pool with 4 workers each holding the connection for
    // 200ms. With a single-connection pool, total time would be ≥800ms
    // (serialised). With 4 connections, total time should be ~200ms.
    let pool = Arc::new(init_db_pool(":memory:", 4).unwrap());
    let start = Instant::now();

    let mut handles = Vec::new();
    for i in 0..4 {
        let p = pool.clone();
        handles.push(tokio::task::spawn_blocking(move || {
            p.query(|_c| {
                std::thread::sleep(Duration::from_millis(200));
                Ok::<i64, rusqlite::Error>(i * 10)
            })
            .unwrap()
        }));
    }
    let results: Vec<i64> = futures::future::join_all(handles.into_iter().map(|h| async move { h.await.unwrap() }))
        .await;
    let elapsed = start.elapsed();

    assert_eq!(results.len(), 4);
    // Should be ~200ms, definitely < 600ms (half of serialised).
    assert!(
        elapsed < Duration::from_millis(600),
        "4 parallel workers took {:?} — pool is serialising",
        elapsed
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pool_serves_many_concurrent_tasks() {
    // Stress: 50 tasks each doing 5 sequential queries. With a 4-conn pool,
    // they should still complete reasonably quickly without deadlock.
    let pool = Arc::new(init_db_pool(":memory:", 4).unwrap());

    let mut handles = Vec::new();
    for _ in 0..50 {
        let p = pool.clone();
        handles.push(tokio::task::spawn_blocking(move || {
            for _ in 0..5 {
                let _ = p.query(|c| {
                    // Write a row to exercise the WAL-mode connection.
                    c.execute("CREATE TABLE IF NOT EXISTS stress (n INTEGER)", [])?;
                    c.execute("INSERT INTO stress (n) VALUES (1)", [])?;
                    Ok::<(), rusqlite::Error>(())
                });
            }
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
}

#[test]
fn pool_capacity_at_least_one() {
    // init_db_pool clamps size to max(1).
    let pool = init_db_pool(":memory:", 0).unwrap();
    assert_eq!(pool.capacity(), 1);
    let pool = init_db_pool(":memory:", 100).unwrap();
    assert_eq!(pool.capacity(), 100);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_reads_dont_deadlock() {
    // Pool with 1 connection and 2 concurrent workers: the second worker
    // waits for the first to return the connection. Make sure the condvar
    // wait doesn't deadlock when a worker holds the connection longer than
    // the other can wait.
    let pool = Arc::new(init_db_pool(":memory:", 1).unwrap());
    let p1 = pool.clone();
    let p2 = pool.clone();

    let h1 = tokio::task::spawn_blocking(move || -> Result<(), String> {
        p1.query(|_c| {
            std::thread::sleep(Duration::from_millis(100));
            Ok::<(), rusqlite::Error>(())
        })
        .map_err(|e| e.to_string())
    });
    let h2 = tokio::task::spawn_blocking(move || -> Result<(), String> {
        std::thread::sleep(Duration::from_millis(20));
        p2.query(|_c| Ok::<(), rusqlite::Error>(())).map_err(|e| e.to_string())
    });

    let r1 = tokio::time::timeout(Duration::from_secs(5), h1).await;
    let r2 = tokio::time::timeout(Duration::from_secs(5), h2).await;
    assert!(r1.is_ok(), "h1 timed out");
    assert!(r2.is_ok(), "h2 timed out");
    let _r1: Result<Result<(), String>, tokio::task::JoinError> = r1.unwrap();
    let _r2: Result<Result<(), String>, tokio::task::JoinError> = r2.unwrap();
}
use std::time::Duration;

#[tokio::test]
async fn daily_logger_creates_logs_directory() {
    use magiclaw::infrastructure::daily_logger::DailyLogger;
    
    let log_dir = "/tmp/magiclaw_test_logs_daily";
    let _ = std::fs::remove_dir_all(log_dir);
    
    // Create logger
    let logger = DailyLogger::new(log_dir).expect("create logger");
    
    // Log some events
    logger.log_token_refresh("peer_123", 150, "long-poll");
    logger.log_send_failure("peer_456", -2, "context_token stale");
    logger.log_session_expired(3, 5);
    logger.log_probe_error("timeout on getupdates");
    
    // Wait a bit to ensure writes complete
    tokio::time::sleep(Duration::from_millis(100)).await;
    
    // Check that log files exist
    assert!(std::path::Path::new(log_dir).exists(), "log directory should exist");
    
    // Check that today's log file exists
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let log_file = format!("{}/magiclaw-{}.log", log_dir, today);
    assert!(std::path::Path::new(&log_file).exists(), "today's log file should exist at {}", log_file);
    
    // Check that log file contains our entries
    let content = std::fs::read_to_string(&log_file).expect("read log file");
    assert!(content.contains("TOKEN_REFRESH"), "should contain TOKEN_REFRESH event");
    assert!(content.contains("peer_123"), "should contain peer_123");
    assert!(content.contains("long-poll"), "should contain long-poll source");
    assert!(content.contains("SEND_FAILURE"), "should contain SEND_FAILURE event");
    assert!(content.contains("SESSION_EXPIRED"), "should contain SESSION_EXPIRED event");
    assert!(content.contains("PROBE_ERROR"), "should contain PROBE_ERROR event");
    
    println!("✓ Daily logger creates and writes to log files");
    println!("  Log file: {}", log_file);
    println!("  Content: {}", content);
    
    // Clean up
    let _ = std::fs::remove_dir_all(log_dir);
}

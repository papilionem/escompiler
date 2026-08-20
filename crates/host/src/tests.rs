#[cfg(test)]
use super::*;

// === BareHost basics ===

#[test]
fn test_bare_host_creation() {
    let host = BareHost::new();
    assert_eq!(host.name(), "bare");
}

#[test]
fn test_bare_host_default() {
    let host = BareHost::default();
    assert_eq!(host.name(), "bare");
}

#[test]
fn test_bare_host_name() {
    let host = BareHost::new();
    assert_eq!(host.name(), "bare");
}

#[test]
fn test_bare_host_description() {
    let host = BareHost::new();
    assert_eq!(host.description(), "Bare host (console only)");
}

#[test]
fn test_bare_host_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<BareHost>();
}

// === format_value ===

#[test]
fn test_format_value_undefined() {
    assert_eq!(format_value(&JsValue::undefined()), "undefined");
}

#[test]
fn test_format_value_null() {
    assert_eq!(format_value(&JsValue::null()), "null");
}

#[test]
fn test_format_value_bool_true() {
    assert_eq!(format_value(&JsValue::bool(true)), "true");
}

#[test]
fn test_format_value_bool_false() {
    assert_eq!(format_value(&JsValue::bool(false)), "false");
}

#[test]
fn test_format_value_int() {
    assert_eq!(format_value(&JsValue::int(42)), "42");
    assert_eq!(format_value(&JsValue::int(0)), "0");
    assert_eq!(format_value(&JsValue::int(-1)), "-1");
}

#[test]
fn test_format_value_float() {
    assert_eq!(format_value(&JsValue::number(2.5)), "2.5");
}

#[test]
fn test_format_value_integer_float() {
    // Whole-number floats should display without decimal
    assert_eq!(format_value(&JsValue::number(5.0)), "5");
    assert_eq!(format_value(&JsValue::number(-100.0)), "-100");
}

#[test]
fn test_format_value_nan() {
    assert_eq!(format_value(&JsValue::number(f64::NAN)), "NaN");
}

#[test]
fn test_format_value_infinity() {
    assert_eq!(format_value(&JsValue::number(f64::INFINITY)), "Infinity");
    assert_eq!(
        format_value(&JsValue::number(f64::NEG_INFINITY)),
        "-Infinity"
    );
}

#[test]
fn test_format_value_object() {
    assert_eq!(
        format_value(&JsValue::object(std::ptr::null())),
        "[object Object]"
    );
}

#[test]
fn test_format_value_string() {
    assert_eq!(format_value(&JsValue::string(std::ptr::null())), "[string]");
}

#[test]
fn test_format_value_symbol() {
    assert_eq!(format_value(&JsValue::symbol(0)), "Symbol()");
}

#[test]
fn test_host_trait_default_description() {
    // The default description() should return name()
    let host = BareHost::new();
    // BareHost overrides description, so test via the trait
    let h: &dyn Host = &host;
    assert_eq!(h.name(), "bare");
    assert_eq!(h.description(), "Bare host (console only)");
}

#[test]
fn test_stdout_console_creation() {
    // StdoutConsole is a unit struct, just verify it exists
    let _console = StdoutConsole;
}

// === HostError display formatting ===

#[test]
fn test_host_error_io_display() {
    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
    let err = HostError::Io(io_err);
    assert_eq!(err.to_string(), "I/O error: file missing");
}

#[test]
fn test_host_error_not_supported_display() {
    let err = HostError::NotSupported("fd_open".to_string());
    assert_eq!(err.to_string(), "not supported: fd_open");
}

#[test]
fn test_host_error_permission_denied_display() {
    let err = HostError::PermissionDenied("/etc/shadow".to_string());
    assert_eq!(err.to_string(), "permission denied: /etc/shadow");
}

#[test]
fn test_host_error_not_found_display() {
    let err = HostError::NotFound("/missing/file".to_string());
    assert_eq!(err.to_string(), "not found: /missing/file");
}

#[test]
fn test_host_error_invalid_argument_display() {
    let err = HostError::InvalidArgument("negative fd".to_string());
    assert_eq!(err.to_string(), "invalid argument: negative fd");
}

#[test]
fn test_host_error_from_io_error() {
    let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
    let host_err: HostError = io_err.into();
    assert!(matches!(host_err, HostError::Io(_)));
}

// === HostStat and SpawnResult construction ===

#[test]
fn test_host_stat_construction() {
    let stat = HostStat {
        size: 1024,
        is_file: true,
        is_dir: false,
        modified_ms: 1700000000000.0,
    };
    assert_eq!(stat.size, 1024);
    assert!(stat.is_file);
    assert!(!stat.is_dir);
    assert!((stat.modified_ms - 1700000000000.0).abs() < f64::EPSILON);
}

#[test]
fn test_host_stat_directory() {
    let stat = HostStat {
        size: 0,
        is_file: false,
        is_dir: true,
        modified_ms: 0.0,
    };
    assert!(!stat.is_file);
    assert!(stat.is_dir);
}

#[test]
fn test_spawn_result_construction() {
    let result = SpawnResult {
        exit_code: 0,
        stdout: b"hello\n".to_vec(),
        stderr: Vec::new(),
    };
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stdout, b"hello\n");
    assert!(result.stderr.is_empty());
}

#[test]
fn test_spawn_result_with_error() {
    let result = SpawnResult {
        exit_code: 1,
        stdout: Vec::new(),
        stderr: b"error: not found\n".to_vec(),
    };
    assert_eq!(result.exit_code, 1);
    assert!(result.stdout.is_empty());
    assert_eq!(result.stderr, b"error: not found\n");
}

// === BareHost HostPrimitives (all return NotSupported) ===

#[test]
fn test_bare_host_fd_open_not_supported() {
    let host = BareHost::new();
    let err = host.fd_open(b"/tmp/test", 0, 0).unwrap_err();
    assert!(matches!(err, HostError::NotSupported(_)));
    assert!(err.to_string().contains("fd_open"));
}

#[test]
fn test_bare_host_fd_read_not_supported() {
    let host = BareHost::new();
    let mut buf = [0u8; 16];
    let err = host.fd_read(0, &mut buf).unwrap_err();
    assert!(matches!(err, HostError::NotSupported(_)));
    assert!(err.to_string().contains("fd_read"));
}

#[test]
fn test_bare_host_fd_write_not_supported() {
    let host = BareHost::new();
    let err = host.fd_write(1, b"hello").unwrap_err();
    assert!(matches!(err, HostError::NotSupported(_)));
    assert!(err.to_string().contains("fd_write"));
}

#[test]
fn test_bare_host_fd_close_not_supported() {
    let host = BareHost::new();
    let err = host.fd_close(3).unwrap_err();
    assert!(matches!(err, HostError::NotSupported(_)));
}

#[test]
fn test_bare_host_fd_stat_not_supported() {
    let host = BareHost::new();
    let err = host.fd_stat(0).unwrap_err();
    assert!(matches!(err, HostError::NotSupported(_)));
}

#[test]
fn test_bare_host_fd_seek_not_supported() {
    let host = BareHost::new();
    let err = host.fd_seek(0, 0, 0).unwrap_err();
    assert!(matches!(err, HostError::NotSupported(_)));
}

#[test]
fn test_bare_host_args_count_zero() {
    let host = BareHost::new();
    assert_eq!(host.args_count(), 0);
}

#[test]
fn test_bare_host_args_get_not_supported() {
    let host = BareHost::new();
    let err = host.args_get(0).unwrap_err();
    assert!(matches!(err, HostError::NotSupported(_)));
}

#[test]
fn test_bare_host_env_get_not_supported() {
    let host = BareHost::new();
    let err = host.env_get("PATH").unwrap_err();
    assert!(matches!(err, HostError::NotSupported(_)));
}

#[test]
fn test_bare_host_spawn_sync_not_supported() {
    let host = BareHost::new();
    let err = host.spawn_sync("ls", &["-la"]).unwrap_err();
    assert!(matches!(err, HostError::NotSupported(_)));
}

#[test]
fn test_bare_host_cwd_not_supported() {
    let host = BareHost::new();
    let err = host.cwd().unwrap_err();
    assert!(matches!(err, HostError::NotSupported(_)));
}

#[test]
fn test_bare_host_isatty_returns_false() {
    let host = BareHost::new();
    assert!(!host.isatty(0));
    assert!(!host.isatty(1));
    assert!(!host.isatty(2));
}

#[test]
fn test_bare_host_now_ms_returns_zero() {
    let host = BareHost::new();
    assert_eq!(host.now_ms(), 0.0);
}

#[test]
fn test_bare_host_hrtime_ns_returns_zero() {
    let host = BareHost::new();
    assert_eq!(host.hrtime_ns(), 0);
}

#[test]
fn test_bare_host_env_set_not_supported() {
    let host = BareHost::new();
    let err = host.env_set("KEY", "value").unwrap_err();
    assert!(matches!(err, HostError::NotSupported(_)));
}

#[test]
fn test_bare_host_chdir_not_supported() {
    let host = BareHost::new();
    let err = host.chdir("/tmp").unwrap_err();
    assert!(matches!(err, HostError::NotSupported(_)));
}

#[test]
fn test_bare_host_fs_mkdir_not_supported() {
    let host = BareHost::new();
    let err = host.fs_mkdir("/tmp/dir", 0o755).unwrap_err();
    assert!(matches!(err, HostError::NotSupported(_)));
}

#[test]
fn test_bare_host_fs_readdir_not_supported() {
    let host = BareHost::new();
    let err = host.fs_readdir("/tmp").unwrap_err();
    assert!(matches!(err, HostError::NotSupported(_)));
}

#[test]
fn test_bare_host_fs_unlink_not_supported() {
    let host = BareHost::new();
    let err = host.fs_unlink("/tmp/file").unwrap_err();
    assert!(matches!(err, HostError::NotSupported(_)));
}

#[test]
fn test_bare_host_fs_rename_not_supported() {
    let host = BareHost::new();
    let err = host.fs_rename("/old", "/new").unwrap_err();
    assert!(matches!(err, HostError::NotSupported(_)));
}

#[test]
fn test_bare_host_fs_exists_returns_false() {
    let host = BareHost::new();
    assert!(!host.fs_exists("/any/path"));
}

// === DefaultHost basic tests ===

#[test]
fn test_default_host_creation() {
    let host = DefaultHost::new();
    assert_eq!(host.name(), "default");
}

#[test]
fn test_default_host_default_trait() {
    let host = DefaultHost::default();
    assert_eq!(host.name(), "default");
}

#[test]
fn test_default_host_description() {
    let host = DefaultHost::new();
    assert_eq!(
        host.description(),
        "Default host (console + real primitives)"
    );
}

#[test]
fn test_default_host_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<DefaultHost>();
}

// === Host trait object with HostPrimitives ===

#[test]
fn test_host_trait_object_includes_primitives() {
    // Verify that a dyn Host can call HostPrimitives methods
    let host = BareHost::new();
    let h: &dyn Host = &host;
    assert_eq!(h.name(), "bare");
    // Can call HostPrimitives methods through the trait object
    assert_eq!(h.args_count(), 0);
    assert!(!h.isatty(0));
}

// === HostStat Clone ===

#[test]
fn test_host_stat_clone() {
    let stat = HostStat {
        size: 42,
        is_file: true,
        is_dir: false,
        modified_ms: 100.0,
    };
    let cloned = stat.clone();
    assert_eq!(cloned.size, 42);
    assert!(cloned.is_file);
}

// === SpawnResult Clone ===

#[test]
fn test_spawn_result_clone() {
    let result = SpawnResult {
        exit_code: 0,
        stdout: b"ok".to_vec(),
        stderr: Vec::new(),
    };
    let cloned = result.clone();
    assert_eq!(cloned.exit_code, 0);
    assert_eq!(cloned.stdout, b"ok");
}

// =========================================================================
// DefaultHost real primitive tests (Step 0.5.18)
// =========================================================================

// --- I/O: fd_open, fd_read, fd_write, fd_close cycle ---

#[test]
fn test_default_host_fd_open_read_write_close_cycle() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    let path_bytes = file_path.to_str().unwrap().as_bytes();

    let host = DefaultHost::new();

    // Open for writing (O_WRONLY | O_CREAT | O_TRUNC)
    let flags = default::O_WRONLY | default::O_CREAT | default::O_TRUNC;
    let fd = host.fd_open(path_bytes, flags, 0o644).unwrap();
    assert!(fd >= 3, "fd should be >= 3, got {fd}");

    // Write some data
    let written = host.fd_write(fd, b"hello world").unwrap();
    assert_eq!(written, 11);

    // Close the write fd
    host.fd_close(fd).unwrap();

    // Open for reading (O_RDONLY)
    let fd2 = host.fd_open(path_bytes, default::O_RDONLY, 0).unwrap();
    assert!(fd2 >= 3);

    // Read the data back
    let mut buf = [0u8; 32];
    let n = host.fd_read(fd2, &mut buf).unwrap();
    assert_eq!(n, 11);
    assert_eq!(&buf[..n], b"hello world");

    // Close the read fd
    host.fd_close(fd2).unwrap();
}

#[test]
fn test_default_host_fd_open_nonexistent_file() {
    let host = DefaultHost::new();
    let result = host.fd_open(b"/nonexistent/path/file.txt", default::O_RDONLY, 0);
    assert!(result.is_err());
}

#[test]
fn test_default_host_fd_open_invalid_utf8() {
    let host = DefaultHost::new();
    let result = host.fd_open(&[0xFF, 0xFE], 0, 0);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, HostError::InvalidArgument(_)));
}

#[test]
fn test_default_host_fd_read_bad_fd() {
    let host = DefaultHost::new();
    let mut buf = [0u8; 16];
    let result = host.fd_read(999, &mut buf);
    assert!(result.is_err());
}

#[test]
fn test_default_host_fd_write_bad_fd() {
    let host = DefaultHost::new();
    let result = host.fd_write(999, b"data");
    assert!(result.is_err());
}

#[test]
fn test_default_host_fd_close_bad_fd() {
    let host = DefaultHost::new();
    let result = host.fd_close(999);
    assert!(result.is_err());
}

#[test]
fn test_default_host_fd_close_stdin_rejected() {
    let host = DefaultHost::new();
    let result = host.fd_close(0);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, HostError::InvalidArgument(_)));
}

#[test]
fn test_default_host_fd_close_stdout_rejected() {
    let host = DefaultHost::new();
    let result = host.fd_close(1);
    assert!(result.is_err());
}

// --- fd_stat ---

#[test]
fn test_default_host_fd_stat_returns_correct_size() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("stat_test.txt");

    // Write known content
    std::fs::write(&file_path, "12345").unwrap();

    let host = DefaultHost::new();
    let path_bytes = file_path.to_str().unwrap().as_bytes();
    let fd = host.fd_open(path_bytes, default::O_RDONLY, 0).unwrap();

    let stat = host.fd_stat(fd).unwrap();
    assert_eq!(stat.size, 5);
    assert!(stat.is_file);
    assert!(!stat.is_dir);
    assert!(stat.modified_ms > 0.0);

    host.fd_close(fd).unwrap();
}

#[test]
fn test_default_host_fd_stat_stdin() {
    let host = DefaultHost::new();
    let stat = host.fd_stat(0).unwrap();
    // stdin has no meaningful metadata
    assert_eq!(stat.size, 0);
}

#[test]
fn test_default_host_fd_stat_bad_fd() {
    let host = DefaultHost::new();
    let result = host.fd_stat(999);
    assert!(result.is_err());
}

// --- fd_seek ---

#[test]
fn test_default_host_fd_seek_to_offset_and_read() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("seek_test.txt");
    std::fs::write(&file_path, "abcdefghij").unwrap();

    let host = DefaultHost::new();
    let path_bytes = file_path.to_str().unwrap().as_bytes();
    let fd = host.fd_open(path_bytes, default::O_RDONLY, 0).unwrap();

    // Seek to offset 5 (SEEK_SET = 0)
    let pos = host.fd_seek(fd, 5, 0).unwrap();
    assert_eq!(pos, 5);

    // Read from offset 5
    let mut buf = [0u8; 16];
    let n = host.fd_read(fd, &mut buf).unwrap();
    assert_eq!(n, 5);
    assert_eq!(&buf[..n], b"fghij");

    host.fd_close(fd).unwrap();
}

#[test]
fn test_default_host_fd_seek_invalid_whence() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("seek_whence.txt");
    std::fs::write(&file_path, "data").unwrap();

    let host = DefaultHost::new();
    let path_bytes = file_path.to_str().unwrap().as_bytes();
    let fd = host.fd_open(path_bytes, default::O_RDONLY, 0).unwrap();

    let result = host.fd_seek(fd, 0, 99);
    assert!(result.is_err());

    host.fd_close(fd).unwrap();
}

#[test]
fn test_default_host_fd_seek_stdin_rejected() {
    let host = DefaultHost::new();
    let result = host.fd_seek(0, 0, 0);
    assert!(result.is_err());
}

// --- Process: args_count, args_get ---

#[test]
fn test_default_host_args_count_positive() {
    let host = DefaultHost::new();
    // The test binary is always at least one arg
    assert!(host.args_count() > 0);
}

#[test]
fn test_default_host_args_get_index_zero() {
    let host = DefaultHost::new();
    // First arg should be the test binary path
    let arg0 = host.args_get(0).unwrap();
    assert!(!arg0.is_empty());
}

#[test]
fn test_default_host_args_get_out_of_range() {
    let host = DefaultHost::new();
    let result = host.args_get(99999);
    assert!(result.is_err());
}

// --- Process: env_get ---

#[test]
fn test_default_host_env_get_known_variable() {
    let host = DefaultHost::new();
    // PATH or HOME should be set in most environments
    let result = host.env_get("PATH").unwrap();
    assert!(result.is_some(), "PATH should be set");
    assert!(!result.unwrap().is_empty());
}

#[test]
fn test_default_host_env_get_nonexistent() {
    let host = DefaultHost::new();
    let result = host
        .env_get("__UNLIKELY_VARIABLE_NAME_FOR_TESTING__")
        .unwrap();
    assert!(result.is_none());
}

// --- Process: spawn_sync ---

#[test]
fn test_default_host_spawn_sync_echo() {
    let host = DefaultHost::new();
    let result = host.spawn_sync("echo", &["hello"]).unwrap();
    assert_eq!(result.exit_code, 0);
    let stdout_str = String::from_utf8_lossy(&result.stdout);
    assert!(
        stdout_str.contains("hello"),
        "stdout should contain 'hello', got: {stdout_str}"
    );
}

#[test]
fn test_default_host_spawn_sync_nonexistent_command() {
    let host = DefaultHost::new();
    let result = host.spawn_sync("__nonexistent_command_for_testing__", &[]);
    assert!(result.is_err());
}

// --- System: cwd ---

#[test]
fn test_default_host_cwd_returns_nonempty() {
    let host = DefaultHost::new();
    let cwd = host.cwd().unwrap();
    assert!(!cwd.is_empty(), "cwd should be non-empty");
}

// --- System: isatty ---

#[test]
fn test_default_host_isatty_returns_bool() {
    let host = DefaultHost::new();
    // We don't assert the specific value since it depends on the test runner,
    // but we verify it doesn't panic
    let _is_tty_0 = host.isatty(0);
    let _is_tty_1 = host.isatty(1);
    let _is_tty_2 = host.isatty(2);
}

#[test]
fn test_default_host_isatty_high_fd_returns_false() {
    let host = DefaultHost::new();
    // A non-existent high fd should not be a tty
    assert!(!host.isatty(9999));
}

// --- Time: now_ms ---

#[test]
fn test_default_host_now_ms_reasonable_value() {
    let host = DefaultHost::new();
    let now = host.now_ms();
    // Should be well past 2023-11-14 (1_700_000_000_000 ms since epoch)
    assert!(
        now > 1_700_000_000_000.0,
        "now_ms should be > 1.7 trillion, got {now}"
    );
}

// --- Time: hrtime_ns ---

#[test]
fn test_default_host_hrtime_ns_increasing() {
    let host = DefaultHost::new();
    let t1 = host.hrtime_ns();
    // Do a tiny bit of work to advance the clock
    let mut sum = 0u64;
    for i in 0..1000 {
        sum = sum.wrapping_add(i);
    }
    // Prevent optimization of the loop
    std::hint::black_box(sum);
    let t2 = host.hrtime_ns();
    assert!(t2 >= t1, "hrtime should be monotonically increasing");
}

// --- Random: random_bytes ---

#[test]
fn test_default_host_random_bytes_fills_buffer() {
    let host = DefaultHost::new();
    let mut buf = [0u8; 32];
    host.random_bytes(&mut buf);
    // It is astronomically unlikely that 32 random bytes are all zero
    let all_zero = buf.iter().all(|&b| b == 0);
    assert!(!all_zero, "random_bytes should fill with non-zero data");
}

#[test]
fn test_default_host_random_bytes_empty_buffer() {
    let host = DefaultHost::new();
    let mut buf = [0u8; 0];
    // Should not panic on empty buffer
    host.random_bytes(&mut buf);
}

#[test]
fn test_default_host_random_bytes_different_calls() {
    let host = DefaultHost::new();
    let mut buf1 = [0u8; 32];
    let mut buf2 = [0u8; 32];
    host.random_bytes(&mut buf1);
    host.random_bytes(&mut buf2);
    // It is astronomically unlikely that two random 32-byte buffers are equal
    assert_ne!(buf1, buf2, "two random fills should differ");
}

// --- SHOULD HAVE: env_set ---

#[test]
fn test_default_host_env_set_and_get() {
    let host = DefaultHost::new();
    host.env_set("__ESC_TEST_VAR__", "test_value").unwrap();
    let val = host.env_get("__ESC_TEST_VAR__").unwrap();
    assert_eq!(val, Some("test_value".to_string()));
}

// --- SHOULD HAVE: fs_mkdir, fs_readdir, fs_unlink, fs_rename, fs_exists ---

#[test]
fn test_default_host_fs_exists() {
    let host = DefaultHost::new();
    // Cargo.toml should exist at the repo root
    assert!(!host.fs_exists("/nonexistent/path/surely"));
}

#[test]
fn test_default_host_fs_mkdir_and_readdir() {
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("subdir");
    let host = DefaultHost::new();

    host.fs_mkdir(sub.to_str().unwrap(), 0o755).unwrap();
    assert!(host.fs_exists(sub.to_str().unwrap()));

    // Create a file in the subdir
    std::fs::write(sub.join("file.txt"), "content").unwrap();

    let entries = host.fs_readdir(sub.to_str().unwrap()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0], "file.txt");
}

#[test]
fn test_default_host_fs_unlink() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("to_delete.txt");
    std::fs::write(&file_path, "data").unwrap();

    let host = DefaultHost::new();
    let path_str = file_path.to_str().unwrap();
    assert!(host.fs_exists(path_str));

    host.fs_unlink(path_str).unwrap();
    assert!(!host.fs_exists(path_str));
}

#[test]
fn test_default_host_fs_rename() {
    let dir = tempfile::tempdir().unwrap();
    let old_path = dir.path().join("old.txt");
    let new_path = dir.path().join("new.txt");
    std::fs::write(&old_path, "data").unwrap();

    let host = DefaultHost::new();
    host.fs_rename(old_path.to_str().unwrap(), new_path.to_str().unwrap())
        .unwrap();

    assert!(!host.fs_exists(old_path.to_str().unwrap()));
    assert!(host.fs_exists(new_path.to_str().unwrap()));
}

// --- SHOULD HAVE: sleep_ms ---

#[test]
fn test_default_host_sleep_ms_does_not_panic() {
    let host = DefaultHost::new();
    // Just verify it doesn't panic with a tiny sleep
    host.sleep_ms(1);
}

// --- fd_write to stdout ---

#[test]
fn test_default_host_fd_write_stdout() {
    let host = DefaultHost::new();
    // Writing to stdout (fd 1) should succeed
    let n = host.fd_write(1, b"").unwrap();
    assert_eq!(n, 0);
}

// --- Open flag constants ---

#[test]
fn test_open_flag_constants() {
    assert_eq!(default::O_RDONLY, 0);
    assert_eq!(default::O_WRONLY, 1);
    assert_eq!(default::O_RDWR, 2);
    assert_eq!(default::O_CREAT, 0x40);
    assert_eq!(default::O_TRUNC, 0x200);
    assert_eq!(default::O_APPEND, 0x400);
}

// =========================================================================
// ABI wrapper tests (Step 0.5.22)
// =========================================================================

#[test]
fn test_abi_fd_write_stdout() {
    let data = b"";
    // SAFETY: empty slice pointer is valid for zero-length operations
    let n = unsafe { abi::__esc_host_fd_write(1, data.as_ptr(), 0) };
    assert_eq!(n, 0);
}

#[test]
fn test_abi_fd_write_null_buf() {
    // Null buffer should return -1
    let n = unsafe { abi::__esc_host_fd_write(1, std::ptr::null(), 5) };
    assert_eq!(n, -1);
}

#[test]
fn test_abi_fd_open_null_path() {
    let fd = unsafe { abi::__esc_host_fd_open(std::ptr::null(), 0, 0, 0) };
    assert_eq!(fd, -1);
}

#[test]
fn test_abi_args_count() {
    let count = abi::__esc_host_args_count();
    assert!(count > 0);
}

#[test]
fn test_abi_now_ms() {
    let now = abi::__esc_host_now_ms();
    assert!(now > 1_700_000_000_000.0);
}

#[test]
fn test_abi_hrtime_ns() {
    let t1 = abi::__esc_host_hrtime_ns();
    let t2 = abi::__esc_host_hrtime_ns();
    assert!(t2 >= t1);
}

#[test]
fn test_abi_isatty() {
    // Non-existent high fd should not be a tty
    let result = abi::__esc_host_isatty(9999);
    assert_eq!(result, 0);
}

#[test]
fn test_abi_random_bytes() {
    let mut buf = [0u8; 16];
    unsafe { abi::__esc_host_random_bytes(buf.as_mut_ptr(), 16) };
    let all_zero = buf.iter().all(|&b| b == 0);
    assert!(!all_zero, "ABI random_bytes should fill buffer");
}

#[test]
fn test_abi_random_bytes_null() {
    // Should not panic on null pointer
    unsafe { abi::__esc_host_random_bytes(std::ptr::null_mut(), 16) };
}

#[test]
fn test_abi_cwd() {
    let mut buf = [0u8; 1024];
    let len = unsafe { abi::__esc_host_cwd(buf.as_mut_ptr(), 1024) };
    assert!(len > 0, "cwd should return positive length");
    let cwd = std::str::from_utf8(&buf[..len as usize]).unwrap();
    assert!(!cwd.is_empty());
}

#[test]
fn test_abi_fd_close_bad_fd() {
    let result = abi::__esc_host_fd_close(9999);
    assert_eq!(result, -1);
}

#[test]
fn test_abi_fd_seek_bad_fd() {
    let result = abi::__esc_host_fd_seek(9999, 0, 0);
    assert_eq!(result, -1);
}

#[test]
fn test_abi_env_get_path() {
    let key = b"PATH";
    let mut buf = [0u8; 4096];
    let len =
        unsafe { abi::__esc_host_env_get(key.as_ptr(), key.len() as u32, buf.as_mut_ptr(), 4096) };
    assert!(len > 0, "PATH should be set, got len={len}");
}

#[test]
fn test_abi_env_get_null_key() {
    let mut buf = [0u8; 64];
    let len = unsafe { abi::__esc_host_env_get(std::ptr::null(), 4, buf.as_mut_ptr(), 64) };
    assert_eq!(len, -1);
}

#[test]
fn test_abi_fd_read_null_buf() {
    let n = unsafe { abi::__esc_host_fd_read(0, std::ptr::null_mut(), 16) };
    assert_eq!(n, -1);
}

#[test]
fn test_abi_fd_stat_size() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("abi_stat.txt");
    std::fs::write(&file_path, "12345678").unwrap();

    let path_str = file_path.to_str().unwrap();
    let path_bytes = path_str.as_bytes();

    // Open via ABI
    let fd = unsafe { abi::__esc_host_fd_open(path_bytes.as_ptr(), path_bytes.len() as u32, 0, 0) };
    assert!(fd >= 3, "fd should be >= 3, got {fd}");

    let size = abi::__esc_host_fd_stat_size(fd);
    assert_eq!(size, 8);

    let close_result = abi::__esc_host_fd_close(fd);
    assert_eq!(close_result, 0);
}

// =========================================================================
// SHOULD HAVE ABI wrapper tests (Step 0.5.19)
// =========================================================================

// --- env_set + env_get roundtrip ---

#[test]
fn test_abi_env_set_and_get_roundtrip() {
    let key = b"__ESC_ABI_TEST_KEY__";
    let val = b"abi_test_value";

    let result = unsafe {
        abi::__esc_host_env_set(
            key.as_ptr(),
            key.len() as u32,
            val.as_ptr(),
            val.len() as u32,
        )
    };
    assert_eq!(result, 0, "env_set should succeed");

    let mut buf = [0u8; 256];
    let len =
        unsafe { abi::__esc_host_env_get(key.as_ptr(), key.len() as u32, buf.as_mut_ptr(), 256) };
    assert!(len > 0, "env_get should return positive length, got {len}");
    let got = std::str::from_utf8(&buf[..len as usize]).unwrap();
    assert_eq!(got, "abi_test_value");
}

#[test]
fn test_abi_env_set_null_key() {
    let val = b"value";
    let result =
        unsafe { abi::__esc_host_env_set(std::ptr::null(), 4, val.as_ptr(), val.len() as u32) };
    assert_eq!(result, -1, "null key should return -1");
}

#[test]
fn test_abi_env_set_null_val() {
    let key = b"KEY";
    let result =
        unsafe { abi::__esc_host_env_set(key.as_ptr(), key.len() as u32, std::ptr::null(), 4) };
    assert_eq!(result, -1, "null val should return -1");
}

// --- chdir ---

#[test]
fn test_abi_chdir_to_tmp_and_back() {
    // Save original cwd
    let mut orig_buf = [0u8; 4096];
    let orig_len = unsafe { abi::__esc_host_cwd(orig_buf.as_mut_ptr(), 4096) };
    assert!(orig_len > 0);
    let orig_cwd = std::str::from_utf8(&orig_buf[..orig_len as usize]).unwrap();

    // chdir to /tmp
    let tmp = b"/tmp";
    let result = unsafe { abi::__esc_host_chdir(tmp.as_ptr(), tmp.len() as u32) };
    assert_eq!(result, 0);

    // Verify cwd changed
    let mut new_buf = [0u8; 4096];
    let new_len = unsafe { abi::__esc_host_cwd(new_buf.as_mut_ptr(), 4096) };
    assert!(new_len > 0);
    let new_cwd = std::str::from_utf8(&new_buf[..new_len as usize]).unwrap();
    assert!(
        new_cwd.contains("tmp"),
        "new cwd should contain 'tmp', got: {new_cwd}"
    );

    // Restore original cwd
    let _ = unsafe { abi::__esc_host_chdir(orig_cwd.as_ptr(), orig_cwd.len() as u32) };
}

#[test]
fn test_abi_chdir_null_path() {
    let result = unsafe { abi::__esc_host_chdir(std::ptr::null(), 4) };
    assert_eq!(result, -1);
}

// --- fs_mkdir + fs_exists + fs_unlink cycle ---

#[test]
fn test_abi_fs_mkdir_exists_unlink_cycle() {
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("abi_test_subdir");
    let sub_str = sub.to_str().unwrap();
    let sub_bytes = sub_str.as_bytes();

    // mkdir
    let result =
        unsafe { abi::__esc_host_fs_mkdir(sub_bytes.as_ptr(), sub_bytes.len() as u32, 0o755) };
    assert_eq!(result, 0, "fs_mkdir should succeed");

    // fs_exists (directory)
    let exists = unsafe { abi::__esc_host_fs_exists(sub_bytes.as_ptr(), sub_bytes.len() as u32) };
    assert_eq!(exists, 1, "directory should exist after mkdir");

    // Create a file inside
    let file_path = sub.join("test_file.txt");
    std::fs::write(&file_path, "data").unwrap();
    let file_str = file_path.to_str().unwrap();
    let file_bytes = file_str.as_bytes();

    // fs_exists (file)
    let exists = unsafe { abi::__esc_host_fs_exists(file_bytes.as_ptr(), file_bytes.len() as u32) };
    assert_eq!(exists, 1, "file should exist after creation");

    // fs_unlink
    let result = unsafe { abi::__esc_host_fs_unlink(file_bytes.as_ptr(), file_bytes.len() as u32) };
    assert_eq!(result, 0, "fs_unlink should succeed");

    // Verify file no longer exists
    let exists = unsafe { abi::__esc_host_fs_exists(file_bytes.as_ptr(), file_bytes.len() as u32) };
    assert_eq!(exists, 0, "file should not exist after unlink");
}

#[test]
fn test_abi_fs_mkdir_null_path() {
    let result = unsafe { abi::__esc_host_fs_mkdir(std::ptr::null(), 4, 0o755) };
    assert_eq!(result, -1);
}

#[test]
fn test_abi_fs_exists_nonexistent() {
    let path = b"/nonexistent/path/unlikely";
    let exists = unsafe { abi::__esc_host_fs_exists(path.as_ptr(), path.len() as u32) };
    assert_eq!(exists, 0);
}

#[test]
fn test_abi_fs_exists_null_path() {
    let exists = unsafe { abi::__esc_host_fs_exists(std::ptr::null(), 4) };
    assert_eq!(exists, 0);
}

#[test]
fn test_abi_fs_unlink_nonexistent() {
    let path = b"/nonexistent/file.txt";
    let result = unsafe { abi::__esc_host_fs_unlink(path.as_ptr(), path.len() as u32) };
    assert_eq!(result, -1, "unlinking non-existent file should return -1");
}

// --- fs_readdir ---

#[test]
fn test_abi_fs_readdir_returns_entries() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("alpha.txt"), "a").unwrap();
    std::fs::write(dir.path().join("beta.txt"), "b").unwrap();

    let path_str = dir.path().to_str().unwrap();
    let path_bytes = path_str.as_bytes();

    let mut buf = [0u8; 4096];
    let len = unsafe {
        abi::__esc_host_fs_readdir(
            path_bytes.as_ptr(),
            path_bytes.len() as u32,
            buf.as_mut_ptr(),
            4096,
        )
    };
    assert!(len > 0, "readdir should return positive length, got {len}");

    let data = std::str::from_utf8(&buf[..len as usize]).unwrap();
    let entries: Vec<&str> = data.split('\0').collect();
    assert_eq!(entries.len(), 2, "should have 2 entries, got: {entries:?}");
    assert!(entries.contains(&"alpha.txt"));
    assert!(entries.contains(&"beta.txt"));
}

#[test]
fn test_abi_fs_readdir_null_path() {
    let mut buf = [0u8; 256];
    let len = unsafe { abi::__esc_host_fs_readdir(std::ptr::null(), 4, buf.as_mut_ptr(), 256) };
    assert_eq!(len, -1);
}

// --- fs_rename ---

#[test]
fn test_abi_fs_rename_moves_file() {
    let dir = tempfile::tempdir().unwrap();
    let old = dir.path().join("old_name.txt");
    let new = dir.path().join("new_name.txt");
    std::fs::write(&old, "content").unwrap();

    let old_str = old.to_str().unwrap();
    let old_bytes = old_str.as_bytes();
    let new_str = new.to_str().unwrap();
    let new_bytes = new_str.as_bytes();

    let result = unsafe {
        abi::__esc_host_fs_rename(
            old_bytes.as_ptr(),
            old_bytes.len() as u32,
            new_bytes.as_ptr(),
            new_bytes.len() as u32,
        )
    };
    assert_eq!(result, 0, "fs_rename should succeed");

    // Old path should not exist
    let exists_old =
        unsafe { abi::__esc_host_fs_exists(old_bytes.as_ptr(), old_bytes.len() as u32) };
    assert_eq!(exists_old, 0, "old path should not exist after rename");

    // New path should exist
    let exists_new =
        unsafe { abi::__esc_host_fs_exists(new_bytes.as_ptr(), new_bytes.len() as u32) };
    assert_eq!(exists_new, 1, "new path should exist after rename");

    // Verify content preserved
    let content = std::fs::read_to_string(&new).unwrap();
    assert_eq!(content, "content");
}

#[test]
fn test_abi_fs_rename_null_old() {
    let new_path = b"/tmp/some_new";
    let result = unsafe {
        abi::__esc_host_fs_rename(
            std::ptr::null(),
            4,
            new_path.as_ptr(),
            new_path.len() as u32,
        )
    };
    assert_eq!(result, -1);
}

// --- sleep_ms ---

#[test]
fn test_abi_sleep_ms_does_not_panic() {
    // Just verify it doesn't crash with a tiny sleep
    abi::__esc_host_sleep_ms(1);
}

#[test]
fn test_abi_sleep_ms_zero() {
    // Zero-duration sleep should be instant and not panic
    abi::__esc_host_sleep_ms(0);
}

// --- Console output via fd_write ---

#[test]
fn test_abi_fd_write_stdout_string() {
    // Write a small string to stdout via fd_write
    let data = b"host abi test output\n";
    let n = unsafe { abi::__esc_host_fd_write(1, data.as_ptr(), data.len() as u32) };
    assert_eq!(n, data.len() as i32, "should write all bytes");
}

#[test]
fn test_abi_fd_write_stderr_string() {
    // Write a small string to stderr via fd_write
    let data = b"host abi stderr test\n";
    let n = unsafe { abi::__esc_host_fd_write(2, data.as_ptr(), data.len() as u32) };
    assert_eq!(n, data.len() as i32, "should write all bytes to stderr");
}

// =========================================================================
// Permission system tests (Step 0.6.20)
// =========================================================================

use crate::permissions::{
    self, PermissionError, PermissionKind, PermissionValue, PermissionsConfig,
};

// --- PermissionKind ---

#[test]
fn test_permission_kind_flag_names() {
    assert_eq!(PermissionKind::Read.flag_name(), "read");
    assert_eq!(PermissionKind::Write.flag_name(), "write");
    assert_eq!(PermissionKind::Net.flag_name(), "net");
    assert_eq!(PermissionKind::Env.flag_name(), "env");
    assert_eq!(PermissionKind::Run.flag_name(), "run");
}

#[test]
fn test_permission_kind_display() {
    assert_eq!(format!("{}", PermissionKind::Read), "read");
    assert_eq!(format!("{}", PermissionKind::Write), "write");
    assert_eq!(format!("{}", PermissionKind::Net), "net");
    assert_eq!(format!("{}", PermissionKind::Env), "env");
    assert_eq!(format!("{}", PermissionKind::Run), "run");
}

// --- PermissionValue::allows ---

#[test]
fn test_permission_value_granted_allows_anything() {
    let perm = PermissionValue::Granted;
    assert!(perm.allows("/etc/passwd", PermissionKind::Read));
    assert!(perm.allows("any-host.com", PermissionKind::Net));
    assert!(perm.allows("PATH", PermissionKind::Env));
}

#[test]
fn test_permission_value_denied_blocks_everything() {
    let perm = PermissionValue::Denied;
    assert!(!perm.allows("/etc/passwd", PermissionKind::Read));
    assert!(!perm.allows("any-host.com", PermissionKind::Net));
    assert!(!perm.allows("PATH", PermissionKind::Env));
}

#[test]
fn test_permission_value_restricted_path_prefix() {
    let perm = PermissionValue::Restricted(vec!["/tmp".to_string(), "/home/user".to_string()]);
    assert!(perm.allows("/tmp/file.txt", PermissionKind::Read));
    assert!(perm.allows("/tmp", PermissionKind::Read));
    assert!(perm.allows("/home/user/data", PermissionKind::Write));
    assert!(!perm.allows("/etc/passwd", PermissionKind::Read));
    assert!(!perm.allows("/home/other", PermissionKind::Read));
}

#[test]
fn test_permission_value_restricted_env_exact_match() {
    let perm = PermissionValue::Restricted(vec!["PATH".to_string(), "HOME".to_string()]);
    assert!(perm.allows("PATH", PermissionKind::Env));
    assert!(perm.allows("HOME", PermissionKind::Env));
    assert!(!perm.allows("SECRET", PermissionKind::Env));
    assert!(!perm.allows("PATHS", PermissionKind::Env)); // not a prefix match
}

#[test]
fn test_permission_value_restricted_run_exact_match() {
    let perm = PermissionValue::Restricted(vec!["node".to_string(), "deno".to_string()]);
    assert!(perm.allows("node", PermissionKind::Run));
    assert!(perm.allows("deno", PermissionKind::Run));
    assert!(!perm.allows("rm", PermissionKind::Run));
}

#[test]
fn test_permission_value_restricted_net_exact_match() {
    let perm = PermissionValue::Restricted(vec!["localhost:8080".to_string()]);
    assert!(perm.allows("localhost:8080", PermissionKind::Net));
    assert!(!perm.allows("evil.com", PermissionKind::Net));
}

// --- PermissionsConfig ---

#[test]
fn test_permissions_config_default_all_granted() {
    let config = PermissionsConfig::new();
    assert_eq!(config.allow_read, PermissionValue::Granted);
    assert_eq!(config.allow_write, PermissionValue::Granted);
    assert_eq!(config.allow_net, PermissionValue::Granted);
    assert_eq!(config.allow_env, PermissionValue::Granted);
    assert_eq!(config.allow_run, PermissionValue::Granted);
}

#[test]
fn test_permissions_config_all_denied() {
    let config = PermissionsConfig::all_denied();
    assert_eq!(config.allow_read, PermissionValue::Denied);
    assert_eq!(config.allow_write, PermissionValue::Denied);
    assert_eq!(config.allow_net, PermissionValue::Denied);
    assert_eq!(config.allow_env, PermissionValue::Denied);
    assert_eq!(config.allow_run, PermissionValue::Denied);
}

#[test]
fn test_permissions_config_get_returns_correct_value() {
    let config = PermissionsConfig {
        allow_read: PermissionValue::Granted,
        allow_write: PermissionValue::Denied,
        allow_net: PermissionValue::Restricted(vec!["localhost".to_string()]),
        allow_env: PermissionValue::Granted,
        allow_run: PermissionValue::Denied,
    };
    assert_eq!(*config.get(PermissionKind::Read), PermissionValue::Granted);
    assert_eq!(*config.get(PermissionKind::Write), PermissionValue::Denied);
    assert_eq!(
        *config.get(PermissionKind::Net),
        PermissionValue::Restricted(vec!["localhost".to_string()])
    );
    assert_eq!(*config.get(PermissionKind::Env), PermissionValue::Granted);
    assert_eq!(*config.get(PermissionKind::Run), PermissionValue::Denied);
}

#[test]
fn test_permissions_config_check_granted() {
    let config = PermissionsConfig::new();
    assert!(config.check(PermissionKind::Read, "/etc/passwd").is_ok());
    assert!(config.check(PermissionKind::Write, "/tmp/file").is_ok());
    assert!(config.check(PermissionKind::Env, "PATH").is_ok());
    assert!(config.check(PermissionKind::Run, "node").is_ok());
    assert!(config.check(PermissionKind::Net, "example.com").is_ok());
}

#[test]
fn test_permissions_config_check_denied_returns_e701() {
    let config = PermissionsConfig::all_denied();
    let err = config
        .check(PermissionKind::Read, "/etc/passwd")
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("ESC-E701"),
        "error should contain ESC-E701: {msg}"
    );
    assert!(
        msg.contains("--allow-read"),
        "error should suggest --allow-read: {msg}"
    );
    assert!(
        msg.contains("/etc/passwd"),
        "error should mention resource: {msg}"
    );
}

#[test]
fn test_permissions_config_check_restricted_allowed() {
    let config = PermissionsConfig {
        allow_read: PermissionValue::Restricted(vec!["/tmp".to_string()]),
        ..PermissionsConfig::all_denied()
    };
    assert!(config.check(PermissionKind::Read, "/tmp/file.txt").is_ok());
}

#[test]
fn test_permissions_config_check_restricted_denied() {
    let config = PermissionsConfig {
        allow_read: PermissionValue::Restricted(vec!["/tmp".to_string()]),
        ..PermissionsConfig::all_denied()
    };
    let err = config
        .check(PermissionKind::Read, "/etc/passwd")
        .unwrap_err();
    assert!(err.to_string().contains("ESC-E701"));
}

// --- Thread-local permission state ---

#[test]
fn test_init_and_check_permissions() {
    let config = PermissionsConfig {
        allow_read: PermissionValue::Granted,
        allow_write: PermissionValue::Denied,
        allow_net: PermissionValue::Denied,
        allow_env: PermissionValue::Granted,
        allow_run: PermissionValue::Denied,
    };
    permissions::init_permissions(config.clone());

    assert!(permissions::check_permission(PermissionKind::Read, "/tmp/file").is_ok());
    assert!(permissions::check_permission(PermissionKind::Write, "/tmp/file").is_err());
    assert!(permissions::check_permission(PermissionKind::Env, "HOME").is_ok());
    assert!(permissions::check_permission(PermissionKind::Run, "node").is_err());

    let current = permissions::current_permissions();
    assert_eq!(current, config);

    // Reset to default for other tests
    permissions::init_permissions(PermissionsConfig::new());
}

// --- PermissionError display ---

#[test]
fn test_permission_error_display() {
    let err = PermissionError::Denied {
        operation: "read /etc/passwd".to_string(),
        kind: PermissionKind::Read,
    };
    let msg = err.to_string();
    assert_eq!(
        msg,
        "ESC-E701: Permission denied: read /etc/passwd requires --allow-read"
    );
}

#[test]
fn test_permission_error_display_write() {
    let err = PermissionError::Denied {
        operation: "write to /var/log/app.log".to_string(),
        kind: PermissionKind::Write,
    };
    let msg = err.to_string();
    assert!(msg.contains("--allow-write"));
    assert!(msg.contains("ESC-E701"));
}

// --- ABI __esc_rt_check_permission ---

#[test]
fn test_abi_check_permission_granted_by_default() {
    // Default is all granted
    permissions::init_permissions(PermissionsConfig::new());
    let resource = b"/tmp/file";
    let result =
        unsafe { abi::__esc_rt_check_permission(0, resource.as_ptr(), resource.len() as u32) };
    assert_eq!(result, 1, "read should be granted by default");
}

#[test]
fn test_abi_check_permission_denied() {
    permissions::init_permissions(PermissionsConfig::all_denied());
    let resource = b"/tmp/file";
    let result =
        unsafe { abi::__esc_rt_check_permission(0, resource.as_ptr(), resource.len() as u32) };
    assert_eq!(result, 0, "read should be denied");
    // Reset
    permissions::init_permissions(PermissionsConfig::new());
}

#[test]
fn test_abi_check_permission_invalid_kind() {
    let resource = b"/tmp";
    let result =
        unsafe { abi::__esc_rt_check_permission(99, resource.as_ptr(), resource.len() as u32) };
    assert_eq!(result, 0, "invalid kind should return 0");
}

#[test]
fn test_abi_check_permission_null_resource() {
    let result = unsafe { abi::__esc_rt_check_permission(0, std::ptr::null(), 0) };
    assert_eq!(
        result, 1,
        "null resource with granted perms should return 1"
    );
}

// --- ABI __esc_rt_init_permissions ---

#[test]
fn test_abi_init_permissions() {
    abi::__esc_rt_init_permissions(1, 0, 1, 0, 1);
    let perms = permissions::current_permissions();
    assert_eq!(perms.allow_read, PermissionValue::Granted);
    assert_eq!(perms.allow_write, PermissionValue::Denied);
    assert_eq!(perms.allow_net, PermissionValue::Granted);
    assert_eq!(perms.allow_env, PermissionValue::Denied);
    assert_eq!(perms.allow_run, PermissionValue::Granted);
    // Reset
    permissions::init_permissions(PermissionsConfig::new());
}

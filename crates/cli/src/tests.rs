use super::*;
use clap::{CommandFactory, Parser};
use common::Edition;
use driver::{CompileMode, CompileTarget, EmitKind};

// --- Cli::try_parse_from tests ---

#[test]
fn test_parse_build_minimal() {
    let cli = Cli::try_parse_from(["esc", "build", "hello.js"]).unwrap();
    match cli.command {
        Commands::Build {
            input,
            output,
            release,
            emit,
            heap_only,
            time_phases,
            edition,
            config_path,
            no_config,
            allow_ffi,
            no_ffi,
            no_eval,
            no_jit,
            allow_read,
            allow_write,
            allow_net,
            allow_env,
            allow_run,
            allow_all,
        } => {
            assert_eq!(input, vec!["hello.js"]);
            assert!(output.is_none());
            assert!(!release);
            assert!(emit.is_none());
            assert!(!heap_only);
            assert!(!time_phases);
            assert_eq!(edition, "es2025");
            assert!(config_path.is_none());
            assert!(!no_config);
            assert!(!allow_ffi);
            assert!(!no_ffi);
            assert!(!no_eval);
            assert!(!no_jit);
            assert!(allow_read.is_none());
            assert!(allow_write.is_none());
            assert!(allow_net.is_none());
            assert!(allow_env.is_none());
            assert!(allow_run.is_none());
            assert!(!allow_all);
        }
        _ => panic!("expected Build command"),
    }
}

#[test]
fn test_parse_build_with_all_flags() {
    let cli = Cli::try_parse_from([
        "esc",
        "build",
        "a.js",
        "b.ts",
        "-o",
        "out.bin",
        "--release",
        "--emit",
        "ir",
        "--heap-only",
        "--time-phases",
        "--edition",
        "es2020",
        "--config",
        "/my/esc.json",
        "--allow-ffi",
        "--no-eval",
        "--no-jit",
    ])
    .unwrap();
    match cli.command {
        Commands::Build {
            input,
            output,
            release,
            emit,
            heap_only,
            time_phases,
            edition,
            config_path,
            no_config,
            allow_ffi,
            no_ffi,
            no_eval,
            no_jit,
            ..
        } => {
            assert_eq!(input, vec!["a.js", "b.ts"]);
            assert_eq!(output.as_deref(), Some("out.bin"));
            assert!(release);
            assert_eq!(emit.as_deref(), Some("ir"));
            assert!(heap_only);
            assert!(time_phases);
            assert_eq!(edition, "es2020");
            assert_eq!(config_path.as_deref(), Some("/my/esc.json"));
            assert!(!no_config);
            assert!(allow_ffi);
            assert!(!no_ffi);
            assert!(no_eval);
            assert!(no_jit);
        }
        _ => panic!("expected Build command"),
    }
}

#[test]
fn test_parse_run_minimal() {
    let cli = Cli::try_parse_from(["esc", "run", "app.js"]).unwrap();
    match cli.command {
        Commands::Run {
            input,
            args,
            heap_only,
            time_phases,
            edition,
            config_path,
            no_config,
            allow_ffi,
            no_ffi,
            no_eval,
            no_jit,
            allow_read,
            allow_write,
            allow_net,
            allow_env,
            allow_run,
            allow_all,
        } => {
            assert_eq!(input, "app.js");
            assert!(args.is_empty());
            assert!(!heap_only);
            assert!(!time_phases);
            assert_eq!(edition, "es2025");
            assert!(config_path.is_none());
            assert!(!no_config);
            assert!(!allow_ffi);
            assert!(!no_ffi);
            assert!(!no_eval);
            assert!(!no_jit);
            assert!(allow_read.is_none());
            assert!(allow_write.is_none());
            assert!(allow_net.is_none());
            assert!(allow_env.is_none());
            assert!(allow_run.is_none());
            assert!(!allow_all);
        }
        _ => panic!("expected Run command"),
    }
}

#[test]
fn test_parse_run_with_flags() {
    let cli = Cli::try_parse_from([
        "esc",
        "run",
        "app.js",
        "--heap-only",
        "--time-phases",
        "--edition",
        "esnext",
    ])
    .unwrap();
    match cli.command {
        Commands::Run {
            heap_only,
            time_phases,
            edition,
            ..
        } => {
            assert!(heap_only);
            assert!(time_phases);
            assert_eq!(edition, "esnext");
        }
        _ => panic!("expected Run command"),
    }
}

#[test]
fn test_parse_check_subcommand() {
    let cli = Cli::try_parse_from(["esc", "check", "lib.ts"]).unwrap();
    match cli.command {
        Commands::Check { input } => {
            assert_eq!(input, vec!["lib.ts"]);
        }
        _ => panic!("expected Check command"),
    }
}

#[test]
fn test_parse_init_with_name() {
    let cli = Cli::try_parse_from(["esc", "init", "my-app"]).unwrap();
    match cli.command {
        Commands::Init { name } => {
            assert_eq!(name.as_deref(), Some("my-app"));
        }
        _ => panic!("expected Init command"),
    }
}

#[test]
fn test_parse_init_without_name() {
    let cli = Cli::try_parse_from(["esc", "init"]).unwrap();
    match cli.command {
        Commands::Init { name } => {
            assert!(name.is_none());
        }
        _ => panic!("expected Init command"),
    }
}

#[test]
fn test_parse_repl_subcommand() {
    let cli = Cli::try_parse_from(["esc", "repl"]).unwrap();
    assert!(matches!(cli.command, Commands::Repl {}));
}

#[test]
fn test_parse_watch_subcommand() {
    let cli = Cli::try_parse_from(["esc", "watch", "a.js"]).unwrap();
    match cli.command {
        Commands::Watch { input } => {
            assert_eq!(input, vec!["a.js"]);
        }
        _ => panic!("expected Watch command"),
    }
}

#[test]
fn test_parse_test_with_filter() {
    let cli = Cli::try_parse_from(["esc", "test", "--filter", "math"]).unwrap();
    match cli.command {
        Commands::Test { filter } => {
            assert_eq!(filter.as_deref(), Some("math"));
        }
        _ => panic!("expected Test command"),
    }
}

#[test]
fn test_parse_report_rejected() {
    let result = Cli::try_parse_from(["esc", "report", "x.js"]);
    assert!(
        result.is_err(),
        "`report` was deleted from the parser, not reserved — it must be unrecognized"
    );
}

#[test]
fn test_help_lists_implemented_commands_only() {
    let help = Cli::command().render_help().to_string();
    for c in ["build", "run", "check"] {
        assert!(
            help.lines().any(|l| l.trim_start().starts_with(c)),
            "--help must advertise `{c}`"
        );
    }
    for c in ["init", "watch", "repl", "test", "report"] {
        assert!(
            !help.lines().any(|l| l.trim_start().starts_with(c)),
            "--help must not advertise `{c}`"
        );
    }
}

#[test]
fn test_parse_no_subcommand_fails() {
    let result = Cli::try_parse_from(["esc"]);
    assert!(result.is_err());
}

#[test]
fn test_parse_unknown_subcommand_fails() {
    let result = Cli::try_parse_from(["esc", "frobnicate"]);
    assert!(result.is_err());
}

// --- parse_emit_kind tests ---

#[test]
fn test_parse_emit_kind_ast() {
    assert_eq!(parse_emit_kind("ast"), Some(EmitKind::Ast));
}

#[test]
fn test_parse_emit_kind_ir() {
    assert_eq!(parse_emit_kind("ir"), Some(EmitKind::Ir));
}

#[test]
fn test_parse_emit_kind_llvm_ir() {
    assert_eq!(parse_emit_kind("llvm-ir"), Some(EmitKind::LlvmIr));
}

#[test]
fn test_parse_emit_kind_asm() {
    assert_eq!(parse_emit_kind("asm"), Some(EmitKind::Asm));
}

#[test]
fn test_parse_emit_kind_unknown() {
    assert!(parse_emit_kind("wasm").is_none());
    assert!(parse_emit_kind("").is_none());
    assert!(parse_emit_kind("IR").is_none()); // case-sensitive
}

// --- Helper to create a default PermissionsConfig for tests ---

fn default_perms() -> (PermissionsConfig, bool) {
    (PermissionsConfig::new(), false)
}

// --- build_config tests ---

#[test]
fn test_build_config_defaults() {
    let (perms, from_cli) = default_perms();
    let cfg = build_config(
        vec!["hello.js".to_string()],
        None,
        false,
        None,
        false,
        false,
        "es2025",
        None,
        false,
        false,
        false,
        false,
        false,
        perms,
        from_cli,
    );
    assert_eq!(cfg.mode, CompileMode::Debug);
    assert_eq!(cfg.target, CompileTarget::Executable);
    assert_eq!(cfg.input, vec!["hello.js"]);
    assert_eq!(cfg.output, "a.out");
    assert!(cfg.emit.is_none());
    assert!(!cfg.heap_only);
    assert!(!cfg.time_phases);
    assert_eq!(cfg.edition, Edition::ES2025);
    assert!(cfg.esc_config.is_none());
    assert!(!cfg.source_map);
    assert!(cfg.out_dir.is_none());
    assert!(cfg.config_path.is_none());
    assert!(!cfg.no_config);
    assert!(!cfg.allow_ffi);
    assert!(cfg.ffi_flag.is_none());
    assert!(cfg.allow_eval);
    assert!(cfg.allow_jit);
    assert_eq!(cfg.permissions, PermissionsConfig::new());
    assert!(!cfg.permissions_from_cli);
}

#[test]
fn test_build_config_release_mode() {
    let (perms, from_cli) = default_perms();
    let cfg = build_config(
        vec!["a.js".to_string()],
        None,
        true,
        None,
        false,
        false,
        "es2025",
        None,
        false,
        false,
        false,
        false,
        false,
        perms,
        from_cli,
    );
    assert_eq!(cfg.mode, CompileMode::Release);
}

#[test]
fn test_build_config_custom_output() {
    let (perms, from_cli) = default_perms();
    let cfg = build_config(
        vec!["a.js".to_string()],
        Some("mybin".to_string()),
        false,
        None,
        false,
        false,
        "es2025",
        None,
        false,
        false,
        false,
        false,
        false,
        perms,
        from_cli,
    );
    assert_eq!(cfg.output, "mybin");
}

#[test]
fn test_build_config_emit_ir() {
    let (perms, from_cli) = default_perms();
    let cfg = build_config(
        vec!["a.js".to_string()],
        None,
        false,
        Some("ir".to_string()),
        false,
        false,
        "es2025",
        None,
        false,
        false,
        false,
        false,
        false,
        perms,
        from_cli,
    );
    assert_eq!(cfg.emit, Some(EmitKind::Ir));
}

#[test]
fn test_build_config_emit_invalid_ignored() {
    let (perms, from_cli) = default_perms();
    let cfg = build_config(
        vec!["a.js".to_string()],
        None,
        false,
        Some("garbage".to_string()),
        false,
        false,
        "es2025",
        None,
        false,
        false,
        false,
        false,
        false,
        perms,
        from_cli,
    );
    assert!(cfg.emit.is_none());
}

#[test]
fn test_build_config_heap_only_and_time_phases() {
    let (perms, from_cli) = default_perms();
    let cfg = build_config(
        vec!["a.js".to_string()],
        None,
        false,
        None,
        true,
        true,
        "es2025",
        None,
        false,
        false,
        false,
        false,
        false,
        perms,
        from_cli,
    );
    assert!(cfg.heap_only);
    assert!(cfg.time_phases);
}

#[test]
fn test_build_config_with_edition() {
    let (perms, from_cli) = default_perms();
    let cfg = build_config(
        vec!["a.js".to_string()],
        None,
        false,
        None,
        false,
        false,
        "es2020",
        None,
        false,
        false,
        false,
        false,
        false,
        perms,
        from_cli,
    );
    assert_eq!(cfg.edition, Edition::ES2020);
}

#[test]
fn test_build_config_invalid_edition_defaults() {
    let (perms, from_cli) = default_perms();
    let cfg = build_config(
        vec!["a.js".to_string()],
        None,
        false,
        None,
        false,
        false,
        "garbage",
        None,
        false,
        false,
        false,
        false,
        false,
        perms,
        from_cli,
    );
    assert_eq!(cfg.edition, Edition::ES2025);
}

#[test]
fn test_build_config_with_config_path() {
    let (perms, from_cli) = default_perms();
    let cfg = build_config(
        vec!["a.js".to_string()],
        None,
        false,
        None,
        false,
        false,
        "es2025",
        Some("/path/to/esc.json".to_string()),
        false,
        false,
        false,
        false,
        false,
        perms,
        from_cli,
    );
    assert_eq!(cfg.config_path.as_deref(), Some("/path/to/esc.json"));
    assert!(!cfg.no_config);
}

#[test]
fn test_build_config_with_no_config() {
    let (perms, from_cli) = default_perms();
    let cfg = build_config(
        vec!["a.js".to_string()],
        None,
        false,
        None,
        false,
        false,
        "es2025",
        None,
        true,
        false,
        false,
        false,
        false,
        perms,
        from_cli,
    );
    assert!(cfg.no_config);
    assert!(cfg.config_path.is_none());
}

#[test]
fn test_build_config_no_eval() {
    let (perms, from_cli) = default_perms();
    let cfg = build_config(
        vec!["a.js".to_string()],
        None,
        false,
        None,
        false,
        false,
        "es2025",
        None,
        false,
        false,
        false,
        true,
        false,
        perms,
        from_cli,
    );
    assert!(!cfg.allow_eval);
    assert!(cfg.allow_jit);
}

#[test]
fn test_build_config_no_jit() {
    let (perms, from_cli) = default_perms();
    let cfg = build_config(
        vec!["a.js".to_string()],
        None,
        false,
        None,
        false,
        false,
        "es2025",
        None,
        false,
        false,
        false,
        false,
        true,
        perms,
        from_cli,
    );
    assert!(cfg.allow_eval);
    assert!(!cfg.allow_jit);
}

// --- run_config tests ---

#[test]
fn test_run_config_defaults() {
    let (perms, from_cli) = default_perms();
    let cfg = run_config(
        "app.js".to_string(),
        false,
        false,
        "es2025",
        None,
        false,
        false,
        false,
        false,
        false,
        perms,
        from_cli,
    );
    assert_eq!(cfg.mode, CompileMode::Debug);
    assert_eq!(cfg.target, CompileTarget::Executable);
    assert_eq!(cfg.input, vec!["app.js"]);
    assert!(cfg.output.is_empty());
    assert!(cfg.emit.is_none());
    assert!(!cfg.heap_only);
    assert!(!cfg.time_phases);
    assert_eq!(cfg.edition, Edition::ES2025);
    assert!(cfg.esc_config.is_none());
    assert!(!cfg.source_map);
    assert!(cfg.out_dir.is_none());
    assert!(!cfg.no_config);
    assert!(!cfg.allow_ffi);
    assert!(cfg.ffi_flag.is_none());
    assert!(cfg.allow_eval);
    assert!(cfg.allow_jit);
    assert_eq!(cfg.permissions, PermissionsConfig::new());
}

#[test]
fn test_run_config_with_flags() {
    let (perms, from_cli) = default_perms();
    let cfg = run_config(
        "app.js".to_string(),
        true,
        true,
        "es2025",
        None,
        false,
        false,
        false,
        false,
        false,
        perms,
        from_cli,
    );
    assert!(cfg.heap_only);
    assert!(cfg.time_phases);
}

#[test]
fn test_run_config_with_edition() {
    let (perms, from_cli) = default_perms();
    let cfg = run_config(
        "app.js".to_string(),
        false,
        false,
        "esnext",
        None,
        false,
        false,
        false,
        false,
        false,
        perms,
        from_cli,
    );
    assert_eq!(cfg.edition, Edition::ESNext);
}

#[test]
fn test_run_config_with_no_config() {
    let (perms, from_cli) = default_perms();
    let cfg = run_config(
        "app.js".to_string(),
        false,
        false,
        "es2025",
        None,
        true,
        false,
        false,
        false,
        false,
        perms,
        from_cli,
    );
    assert!(cfg.no_config);
}

// --- parse_edition tests ---

#[test]
fn test_parse_edition_valid() {
    assert_eq!(parse_edition("es5"), Edition::ES5);
    assert_eq!(parse_edition("es2025"), Edition::ES2025);
    assert_eq!(parse_edition("esnext"), Edition::ESNext);
}

#[test]
fn test_parse_edition_invalid_defaults() {
    assert_eq!(parse_edition("garbage"), Edition::ES2025);
    assert_eq!(parse_edition(""), Edition::ES2025);
}

// --- FFI flag tests ---

#[test]
fn test_parse_build_with_allow_ffi() {
    let cli = Cli::try_parse_from(["esc", "build", "hello.js", "--allow-ffi"]).unwrap();
    match cli.command {
        Commands::Build {
            allow_ffi, no_ffi, ..
        } => {
            assert!(allow_ffi);
            assert!(!no_ffi);
        }
        _ => panic!("expected Build command"),
    }
}

#[test]
fn test_parse_build_with_no_ffi() {
    let cli = Cli::try_parse_from(["esc", "build", "hello.js", "--no-ffi"]).unwrap();
    match cli.command {
        Commands::Build {
            allow_ffi, no_ffi, ..
        } => {
            assert!(!allow_ffi);
            assert!(no_ffi);
        }
        _ => panic!("expected Build command"),
    }
}

#[test]
fn test_parse_build_allow_ffi_and_no_ffi_conflict() {
    let result = Cli::try_parse_from(["esc", "build", "hello.js", "--allow-ffi", "--no-ffi"]);
    assert!(result.is_err());
}

#[test]
fn test_parse_run_with_allow_ffi() {
    let cli = Cli::try_parse_from(["esc", "run", "app.js", "--allow-ffi"]).unwrap();
    match cli.command {
        Commands::Run {
            allow_ffi, no_ffi, ..
        } => {
            assert!(allow_ffi);
            assert!(!no_ffi);
        }
        _ => panic!("expected Run command"),
    }
}

#[test]
fn test_parse_run_with_no_ffi() {
    let cli = Cli::try_parse_from(["esc", "run", "app.js", "--no-ffi"]).unwrap();
    match cli.command {
        Commands::Run {
            allow_ffi, no_ffi, ..
        } => {
            assert!(!allow_ffi);
            assert!(no_ffi);
        }
        _ => panic!("expected Run command"),
    }
}

#[test]
fn test_build_config_allow_ffi() {
    let (perms, from_cli) = default_perms();
    let cfg = build_config(
        vec!["a.js".to_string()],
        None,
        false,
        None,
        false,
        false,
        "es2025",
        None,
        false,
        true,
        false,
        false,
        false,
        perms,
        from_cli,
    );
    assert!(cfg.allow_ffi);
    assert_eq!(cfg.ffi_flag, Some(true));
}

#[test]
fn test_build_config_no_ffi() {
    let (perms, from_cli) = default_perms();
    let cfg = build_config(
        vec!["a.js".to_string()],
        None,
        false,
        None,
        false,
        false,
        "es2025",
        None,
        false,
        false,
        true,
        false,
        false,
        perms,
        from_cli,
    );
    assert!(!cfg.allow_ffi);
    assert_eq!(cfg.ffi_flag, Some(false));
}

#[test]
fn test_build_config_ffi_default_neither_flag() {
    let (perms, from_cli) = default_perms();
    let cfg = build_config(
        vec!["a.js".to_string()],
        None,
        false,
        None,
        false,
        false,
        "es2025",
        None,
        false,
        false,
        false,
        false,
        false,
        perms,
        from_cli,
    );
    assert!(!cfg.allow_ffi);
    assert!(cfg.ffi_flag.is_none());
}

#[test]
fn test_run_config_allow_ffi() {
    let (perms, from_cli) = default_perms();
    let cfg = run_config(
        "app.js".to_string(),
        false,
        false,
        "es2025",
        None,
        false,
        true,
        false,
        false,
        false,
        perms,
        from_cli,
    );
    assert!(cfg.allow_ffi);
    assert_eq!(cfg.ffi_flag, Some(true));
}

#[test]
fn test_run_config_no_ffi() {
    let (perms, from_cli) = default_perms();
    let cfg = run_config(
        "app.js".to_string(),
        false,
        false,
        "es2025",
        None,
        false,
        false,
        true,
        false,
        false,
        perms,
        from_cli,
    );
    assert!(!cfg.allow_ffi);
    assert_eq!(cfg.ffi_flag, Some(false));
}

// --- resolve_ffi_flag tests ---

#[test]
fn test_resolve_ffi_flag_allow() {
    assert_eq!(resolve_ffi_flag(true, false), Some(true));
}

#[test]
fn test_resolve_ffi_flag_no() {
    assert_eq!(resolve_ffi_flag(false, true), Some(false));
}

#[test]
fn test_resolve_ffi_flag_neither() {
    assert_eq!(resolve_ffi_flag(false, false), None);
}

// =========================================================================
// Permission system tests (Step 0.6.20)
// =========================================================================

// --- parse_permission_flag tests ---

#[test]
fn test_parse_permission_flag_none() {
    assert!(parse_permission_flag(None).is_none());
}

#[test]
fn test_parse_permission_flag_empty_grants_all() {
    let perm = parse_permission_flag(Some("")).unwrap();
    assert_eq!(perm, PermissionValue::Granted);
}

#[test]
fn test_parse_permission_flag_single_path() {
    let perm = parse_permission_flag(Some("/tmp")).unwrap();
    assert_eq!(perm, PermissionValue::Restricted(vec!["/tmp".to_string()]));
}

#[test]
fn test_parse_permission_flag_multiple_paths() {
    let perm = parse_permission_flag(Some("/tmp,/home")).unwrap();
    assert_eq!(
        perm,
        PermissionValue::Restricted(vec!["/tmp".to_string(), "/home".to_string()])
    );
}

#[test]
fn test_parse_permission_flag_trims_whitespace() {
    let perm = parse_permission_flag(Some(" /tmp , /home ")).unwrap();
    assert_eq!(
        perm,
        PermissionValue::Restricted(vec!["/tmp".to_string(), "/home".to_string()])
    );
}

// --- build_permissions tests ---

#[test]
fn test_build_permissions_no_flags_all_granted() {
    let (config, from_cli) = build_permissions(None, None, None, None, None, false);
    assert_eq!(config, PermissionsConfig::new());
    assert!(!from_cli);
}

#[test]
fn test_build_permissions_allow_all() {
    let (config, from_cli) = build_permissions(None, None, None, None, None, true);
    assert_eq!(config, PermissionsConfig::new());
    assert!(from_cli);
}

#[test]
fn test_build_permissions_only_read_denies_others() {
    let (config, from_cli) = build_permissions(Some(""), None, None, None, None, false);
    assert!(from_cli);
    assert_eq!(config.allow_read, PermissionValue::Granted);
    assert_eq!(config.allow_write, PermissionValue::Denied);
    assert_eq!(config.allow_net, PermissionValue::Denied);
    assert_eq!(config.allow_env, PermissionValue::Denied);
    assert_eq!(config.allow_run, PermissionValue::Denied);
}

#[test]
fn test_build_permissions_read_and_env() {
    let (config, from_cli) = build_permissions(Some(""), None, None, Some(""), None, false);
    assert!(from_cli);
    assert_eq!(config.allow_read, PermissionValue::Granted);
    assert_eq!(config.allow_write, PermissionValue::Denied);
    assert_eq!(config.allow_net, PermissionValue::Denied);
    assert_eq!(config.allow_env, PermissionValue::Granted);
    assert_eq!(config.allow_run, PermissionValue::Denied);
}

#[test]
fn test_build_permissions_path_restricted() {
    let (config, from_cli) = build_permissions(
        Some("/tmp,/home"),
        Some("/var/log"),
        None,
        None,
        None,
        false,
    );
    assert!(from_cli);
    assert_eq!(
        config.allow_read,
        PermissionValue::Restricted(vec!["/tmp".to_string(), "/home".to_string()])
    );
    assert_eq!(
        config.allow_write,
        PermissionValue::Restricted(vec!["/var/log".to_string()])
    );
}

// --- CLI parsing of permission flags ---

#[test]
fn test_parse_build_with_allow_all() {
    let cli = Cli::try_parse_from(["esc", "build", "app.js", "--allow-all"]).unwrap();
    match cli.command {
        Commands::Build { allow_all, .. } => {
            assert!(allow_all);
        }
        _ => panic!("expected Build command"),
    }
}

#[test]
fn test_parse_build_with_allow_read() {
    let cli = Cli::try_parse_from(["esc", "build", "app.js", "--allow-read"]).unwrap();
    match cli.command {
        Commands::Build { allow_read, .. } => {
            assert_eq!(allow_read.as_deref(), Some(""));
        }
        _ => panic!("expected Build command"),
    }
}

#[test]
fn test_parse_build_with_allow_read_restricted() {
    let cli = Cli::try_parse_from(["esc", "build", "app.js", "--allow-read=/tmp,/home"]).unwrap();
    match cli.command {
        Commands::Build { allow_read, .. } => {
            assert_eq!(allow_read.as_deref(), Some("/tmp,/home"));
        }
        _ => panic!("expected Build command"),
    }
}

#[test]
fn test_parse_run_with_allow_env() {
    let cli = Cli::try_parse_from(["esc", "run", "app.js", "--allow-env"]).unwrap();
    match cli.command {
        Commands::Run { allow_env, .. } => {
            assert_eq!(allow_env.as_deref(), Some(""));
        }
        _ => panic!("expected Run command"),
    }
}

#[test]
fn test_parse_run_with_allow_run_restricted() {
    let cli = Cli::try_parse_from(["esc", "run", "app.js", "--allow-run=node,deno"]).unwrap();
    match cli.command {
        Commands::Run { allow_run, .. } => {
            assert_eq!(allow_run.as_deref(), Some("node,deno"));
        }
        _ => panic!("expected Run command"),
    }
}

// --- PermissionsConfig in CompilerConfig ---

#[test]
fn test_build_config_with_permissions() {
    let perms = PermissionsConfig {
        allow_read: PermissionValue::Granted,
        allow_write: PermissionValue::Denied,
        allow_net: PermissionValue::Denied,
        allow_env: PermissionValue::Granted,
        allow_run: PermissionValue::Denied,
    };
    let cfg = build_config(
        vec!["a.js".to_string()],
        None,
        false,
        None,
        false,
        false,
        "es2025",
        None,
        false,
        false,
        false,
        false,
        false,
        perms.clone(),
        true,
    );
    assert_eq!(cfg.permissions, perms);
    assert!(cfg.permissions_from_cli);
}

#[test]
fn test_run_config_with_permissions() {
    let perms = PermissionsConfig {
        allow_read: PermissionValue::Denied,
        allow_write: PermissionValue::Denied,
        allow_net: PermissionValue::Granted,
        allow_env: PermissionValue::Denied,
        allow_run: PermissionValue::Denied,
    };
    let cfg = run_config(
        "app.js".to_string(),
        false,
        false,
        "es2025",
        None,
        false,
        false,
        false,
        false,
        false,
        perms.clone(),
        true,
    );
    assert_eq!(cfg.permissions, perms);
    assert!(cfg.permissions_from_cli);
}

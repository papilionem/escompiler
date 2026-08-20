//! ESCompiler — JavaScript/TypeScript AOT compiler CLI.

use clap::Parser;
use cli::{Cli, Commands};

#[cfg(not(windows))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

fn main() {
    let cli = Cli::parse();

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
            let (permissions, permissions_from_cli) = cli::build_permissions(
                allow_read.as_deref(),
                allow_write.as_deref(),
                allow_net.as_deref(),
                allow_env.as_deref(),
                allow_run.as_deref(),
                allow_all,
            );
            let config = cli::build_config(
                input,
                output,
                release,
                emit,
                heap_only,
                time_phases,
                &edition,
                config_path,
                no_config,
                allow_ffi,
                no_ffi,
                no_eval,
                no_jit,
                permissions,
                permissions_from_cli,
            );
            if let Err(err) = driver::compile(&config) {
                // exit_code() is 2 for a deliberate refusal, 1 for a genuine
                // failure. Both used to exit 1, which made "this feature does not
                // exist yet" indistinguishable from "your program is broken".
                eprintln!("error: {err}");
                std::process::exit(err.exit_code());
            }
        }
        Commands::Run {
            input,
            args: _,
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
            let (permissions, permissions_from_cli) = cli::build_permissions(
                allow_read.as_deref(),
                allow_write.as_deref(),
                allow_net.as_deref(),
                allow_env.as_deref(),
                allow_run.as_deref(),
                allow_all,
            );
            let config = cli::run_config(
                input,
                heap_only,
                time_phases,
                &edition,
                config_path,
                no_config,
                allow_ffi,
                no_ffi,
                no_eval,
                no_jit,
                permissions,
                permissions_from_cli,
            );
            match driver::run(&config) {
                Ok(code) => std::process::exit(code),
                Err(err) => {
                    eprintln!("error: {err}");
                    std::process::exit(1);
                }
            }
        }
        Commands::Check { input } => {
            let config = driver::CompilerConfig::new(input);
            if let Err(err) = driver::check(&config) {
                eprintln!("error: {err}");
                std::process::exit(1);
            }
        }
        Commands::Init { .. } => {
            eprintln!("esc: error[ESC-E002]: `esc init` is not implemented yet");
            std::process::exit(2);
        }
        Commands::Watch { .. } => {
            eprintln!("esc: error[ESC-E002]: `esc watch` is not implemented yet");
            std::process::exit(2);
        }
        Commands::Repl {} => {
            eprintln!("esc: error[ESC-E002]: `esc repl` is not implemented yet");
            std::process::exit(2);
        }
        Commands::Test { .. } => {
            eprintln!("esc: error[ESC-E002]: `esc test` is not implemented yet");
            std::process::exit(2);
        }
    }
}

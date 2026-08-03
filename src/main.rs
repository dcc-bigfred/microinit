//! microinit — PID 1 init system and service supervisor for BigFred OS.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

use clap::{Parser, Subcommand};
use microinit::cli;
use microinit::config::{
    self, default_config_path, DEFAULT_CONSOLE, DEFAULT_INIT_LOGS_TTY, DEFAULT_LOGS_TTY,
    DEFAULT_SOCKET,
};
use microinit::init;

#[derive(Parser, Debug)]
#[command(
    name = "microinit",
    about = "Init system and service supervisor for BigFred OS",
    long_about = "See microinit(8) for full documentation."
)]
struct Cli {
    /// Path to the control socket (default /run/microinit.sock)
    #[arg(long, global = true, default_value = DEFAULT_SOCKET)]
    socket: PathBuf,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Run as PID 1 / system init (start services, supervise, IPC)
    Init {
        /// TTY for mixed service stdout/stderr logs
        #[arg(long, default_value = DEFAULT_LOGS_TTY)]
        logs_tty: String,
        /// TTY for microinit's own operational logs (start/stop/restart, errors)
        #[arg(long, default_value = DEFAULT_INIT_LOGS_TTY)]
        init_logs_tty: String,
        /// Console TTY for [ OK ]/[ FAIL ] status and getty
        #[arg(long, default_value = DEFAULT_CONSOLE)]
        console: String,
        /// Config file path
        #[arg(long, default_value_os_t = default_config_path())]
        config: PathBuf,
        /// Skip early-boot entirely (local / host testing)
        #[arg(long)]
        no_early_boot: bool,
        /// Allow continuing if early-boot script is missing (host testing)
        #[arg(long)]
        allow_no_early_boot: bool,
        /// Append service/init logs to files under logs.dir (overrides config logToFiles)
        #[arg(long)]
        log_to_files: bool,
    },
    /// Run as container / host supervisor (no early-boot, no getty, no TTYs)
    Supervise {
        /// Console device for [ OK ]/[ FAIL ] status (optional; often /dev/null)
        #[arg(long, default_value = "/dev/null")]
        console: String,
        /// Config file path
        #[arg(long, default_value_os_t = default_config_path())]
        config: PathBuf,
        /// Append service/init logs to files under logs.dir (overrides config logToFiles)
        #[arg(long)]
        log_to_files: bool,
    },
    /// Start a service
    Start { name: String },
    /// Stop a service
    Stop { name: String },
    /// Restart a service
    Restart { name: String },
    /// Enable a service (persist override + start)
    Enable { name: String },
    /// Disable a service (persist override + stop)
    Disable { name: String },
    /// List services and their state
    List,
    /// Show service logs (or mixed if name omitted)
    Logs {
        name: Option<String>,
        /// Follow log stream
        #[arg(long, short = 'f')]
        follow: bool,
        /// Number of historical lines (default from config / 300)
        #[arg(long, short = 'n')]
        lines: Option<usize>,
    },
}

fn paths_for_config(config_path: &Path) -> config::Paths {
    let mut paths = config::Paths {
        config: config_path.to_path_buf(),
        ..config::Paths::default()
    };
    if let Some(parent) = config_path.parent() {
        paths.example = parent.join("microinit.json.example");
        paths.override_file = parent.join("microinit.services.enabled-override.json");
        paths.dropins_dir = parent.join("microinit.d").join("services");
    }
    paths
}

fn default_init_opts(socket: String) -> init::InitOpts {
    init::InitOpts {
        logs_tty: DEFAULT_LOGS_TTY.to_string(),
        init_logs_tty: DEFAULT_INIT_LOGS_TTY.to_string(),
        console: DEFAULT_CONSOLE.to_string(),
        paths: paths_for_config(&default_config_path()),
        skip_early_boot: false,
        require_early_boot: true,
        log_to_files: false,
        spawn_getty: true,
        attach_ttys: true,
        socket,
    }
}

/// PID 1 must never exit: the kernel panics ("Attempted to kill init!").
/// Bypass clap entirely — leftover kernel cmdline args become argv and would
/// make `Cli::parse()` call `exit(2)` before our auto-init fallback.
fn run_as_pid1() -> ! {
    eprintln!("microinit: running as PID 1 (init)");
    let opts = default_init_opts(DEFAULT_SOCKET.to_string());
    if let Err(e) = init::run(opts) {
        eprintln!("microinit: fatal: {e}");
    }
    // Stay alive so the system remains debuggable on the console.
    loop {
        eprintln!("microinit: PID 1 idle after fatal error; not exiting");
        thread::sleep(Duration::from_secs(3600));
    }
}

fn main() -> ExitCode {
    // Kernel may exec /sbin/init with unrecognized cmdline tokens as argv.
    // Never go through clap when we are already PID 1.
    if std::process::id() == 1 {
        run_as_pid1();
    }

    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(e) => {
            // argv0 `init` without a subcommand (or with junk args): still boot.
            if init::should_auto_init() {
                eprintln!("microinit: CLI parse failed ({e}); falling back to init mode");
                let opts = default_init_opts(DEFAULT_SOCKET.to_string());
                return match init::run(opts) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(err) => {
                        eprintln!("microinit: {err}");
                        ExitCode::FAILURE
                    }
                };
            }
            // Normal CLI usage error (prints help / message).
            let _ = e.print();
            return ExitCode::from(2);
        }
    };

    let socket = cli.socket.display().to_string();

    let command = match cli.command {
        Some(c) => c,
        None if init::should_auto_init() => Commands::Init {
            logs_tty: DEFAULT_LOGS_TTY.to_string(),
            init_logs_tty: DEFAULT_INIT_LOGS_TTY.to_string(),
            console: DEFAULT_CONSOLE.to_string(),
            config: default_config_path(),
            no_early_boot: false,
            allow_no_early_boot: false,
            log_to_files: false,
        },
        None => {
            eprintln!("microinit: missing subcommand (try --help)");
            return ExitCode::from(2);
        }
    };

    let result = match command {
        Commands::Init {
            logs_tty,
            init_logs_tty,
            console,
            config: config_path,
            no_early_boot,
            allow_no_early_boot,
            log_to_files,
        } => init::run(init::InitOpts {
            logs_tty,
            init_logs_tty,
            console,
            paths: paths_for_config(&config_path),
            skip_early_boot: no_early_boot,
            require_early_boot: !allow_no_early_boot && !no_early_boot,
            log_to_files,
            spawn_getty: true,
            attach_ttys: true,
            socket,
        }),
        Commands::Supervise {
            console,
            config: config_path,
            log_to_files,
        } => init::run(init::InitOpts {
            logs_tty: DEFAULT_LOGS_TTY.to_string(),
            init_logs_tty: DEFAULT_INIT_LOGS_TTY.to_string(),
            console,
            paths: paths_for_config(&config_path),
            skip_early_boot: true,
            require_early_boot: false,
            log_to_files,
            spawn_getty: false,
            attach_ttys: false,
            socket,
        }),
        Commands::Start { name } => cli::cmd_start(&cli.socket, &name),
        Commands::Stop { name } => cli::cmd_stop(&cli.socket, &name),
        Commands::Restart { name } => cli::cmd_restart(&cli.socket, &name),
        Commands::Enable { name } => cli::cmd_enable(&cli.socket, &name),
        Commands::Disable { name } => cli::cmd_disable(&cli.socket, &name),
        Commands::List => cli::cmd_list(&cli.socket),
        Commands::Logs {
            name,
            follow,
            lines,
        } => cli::cmd_logs(&cli.socket, name, follow, lines),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("microinit: {e}");
            ExitCode::FAILURE
        }
    }
}

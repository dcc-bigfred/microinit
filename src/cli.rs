//! CLI handlers that talk to the running `microinit init` daemon via IPC.

use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::path::Path;

use crate::error::{Error, Result};
use crate::ipc::{read_frame, request, write_frame};
use crate::protocol::{DepNode, Request, Response, ServiceDescribe, ServiceEvent, ShutdownMode};

/// Result of parsing SysV-style `shutdown` argv (excluding `--socket`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownCliMode {
    Help,
    Mode(ShutdownMode),
}

/// Parse SysV-compatible shutdown flags (`-h`/`-P`/`-r`/`-H`, `now`, …).
pub fn parse_shutdown_args(
    args: &[impl AsRef<str>],
) -> std::result::Result<ShutdownCliMode, String> {
    let mut mode = ShutdownMode::Poweroff;
    let mut seen = false;

    for a in args {
        let a = a.as_ref();
        match a {
            "--help" | "help" => return Ok(ShutdownCliMode::Help),
            "-h" | "-P" | "--poweroff" | "poweroff" => {
                if seen && mode != ShutdownMode::Poweroff {
                    return Err("conflicting mode flags".into());
                }
                mode = ShutdownMode::Poweroff;
                seen = true;
            }
            "-r" | "--reboot" | "reboot" => {
                if seen && mode != ShutdownMode::Reboot {
                    return Err("conflicting mode flags".into());
                }
                mode = ShutdownMode::Reboot;
                seen = true;
            }
            "-H" | "--halt" | "halt" => {
                if seen && mode != ShutdownMode::Halt {
                    return Err("conflicting mode flags".into());
                }
                mode = ShutdownMode::Halt;
                seen = true;
            }
            "now" | "+0" | "0" => {
                // SysV compatibility; delayed shutdown is not implemented.
            }
            _ if a.starts_with('-') => {
                return Err(format!("unknown option {a:?}"));
            }
            _ => {
                // Ignore wall message / unsupported time specs.
            }
        }
    }
    Ok(ShutdownCliMode::Mode(mode))
}

/// Ask the daemon to begin ordered shutdown (`poweroff` / `reboot` / `halt`).
pub fn cmd_shutdown(socket: &Path, mode: ShutdownMode) -> Result<()> {
    simple_ok(socket, Request::Shutdown { mode })
}

pub fn cmd_list(socket: &Path) -> Result<()> {
    match request(socket, &Request::List)? {
        Response::List { services } => {
            println!(
                "{:<20} {:<22} {:>8} {:>8} {:>8} {:>10}",
                "NAME", "STATE", "PID", "RESTARTS", "ENABLED", "LIVE_FAIL"
            );
            for s in services {
                let pid = s.pid.map(|p| p.to_string()).unwrap_or_else(|| "-".into());
                println!(
                    "{:<20} {:<22} {:>8} {:>8} {:>8} {:>10}",
                    s.name,
                    s.state.to_string(),
                    pid,
                    s.restarts,
                    if s.enabled { "yes" } else { "no" },
                    s.liveness_failures
                );
            }
            Ok(())
        }
        Response::Error { message } => Err(Error::Ipc(message)),
        other => Err(Error::Ipc(format!("unexpected response: {other:?}"))),
    }
}

pub fn cmd_describe(socket: &Path, name: &str) -> Result<()> {
    match request(socket, &Request::Describe { name: name.into() })? {
        Response::Describe { describe } => {
            print_describe(&describe);
            Ok(())
        }
        Response::Error { message } => Err(Error::Ipc(message)),
        other => Err(Error::Ipc(format!("unexpected response: {other:?}"))),
    }
}

fn print_describe(d: &ServiceDescribe) {
    let s = &d.status;
    let pid = s.pid.map(|p| p.to_string()).unwrap_or_else(|| "-".into());
    println!("Service: {}", s.name);
    println!("State:   {}", s.state);
    println!("PID:     {pid}");
    println!("Enabled: {}", if s.enabled { "yes" } else { "no" });
    println!("Restarts: {}", s.restarts);
    println!("Liveness failures: {}", s.liveness_failures);
    println!("Uptime:  {}", format_uptime(d.uptime_secs));
    println!();

    println!("Depends on:");
    print_dep_list(&d.depends_on);
    println!();

    println!("Required by:");
    print_dep_list(&d.dependents);
    println!();

    println!("Dependency graph:");
    print_dep_graph(&d.dep_nodes, &d.dep_edges);
    println!();

    println!("Recent events (last {}):", d.events.len());
    if d.events.is_empty() {
        println!("  (none)");
    } else {
        for ev in &d.events {
            println!("  {}", format_event(ev));
        }
    }
}

fn print_dep_list(nodes: &[DepNode]) {
    if nodes.is_empty() {
        println!("  (none)");
        return;
    }
    for n in nodes {
        println!("  {} ({})", n.name, n.state);
    }
}

fn format_uptime(secs: Option<u64>) -> String {
    let Some(mut s) = secs else {
        return "-".into();
    };
    let days = s / 86_400;
    s %= 86_400;
    let hours = s / 3_600;
    s %= 3_600;
    let mins = s / 60;
    s %= 60;
    if days > 0 {
        format!("{days}d {hours}h {mins}m")
    } else if hours > 0 {
        format!("{hours}h {mins}m")
    } else if mins > 0 {
        format!("{mins}m {s}s")
    } else {
        format!("{s}s")
    }
}

fn format_event(ev: &ServiceEvent) -> String {
    use crate::protocol::ServiceEventKind;
    match ev.kind {
        ServiceEventKind::StateChange => {
            let from = ev.from.map(|s| s.to_string()).unwrap_or_else(|| "?".into());
            let to = ev.to.map(|s| s.to_string()).unwrap_or_else(|| "?".into());
            format!("{}  state_change  {from} -> {to}", ev.ts)
        }
        ServiceEventKind::Restart => format!("{}  restart", ev.ts),
        ServiceEventKind::LivenessFailed => match &ev.detail {
            Some(d) => format!("{}  liveness_failed  ({d})", ev.ts),
            None => format!("{}  liveness_failed", ev.ts),
        },
    }
}

/// Nested tree from roots (`├─>` / `└─>`), with `[already shown]` for repeats.
fn print_dep_graph(nodes: &[DepNode], edges: &[(String, String)]) {
    if edges.is_empty() {
        if nodes.len() <= 1 {
            println!("  (none)");
            return;
        }
        for n in nodes {
            println!("  {} ({})", n.name, n.state);
        }
        return;
    }

    let states: HashMap<&str, &DepNode> = nodes.iter().map(|n| (n.name.as_str(), n)).collect();
    let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut has_parent: HashSet<&str> = HashSet::new();
    for (from, to) in edges {
        children.entry(from.as_str()).or_default().push(to.as_str());
        has_parent.insert(to.as_str());
    }
    for kids in children.values_mut() {
        kids.sort_unstable();
        kids.dedup();
    }

    let mut roots: Vec<&str> = nodes
        .iter()
        .map(|n| n.name.as_str())
        .filter(|n| !has_parent.contains(n))
        .collect();
    roots.sort_unstable();
    if roots.is_empty() {
        // Cycle covering whole subgraph — pick lex-smallest as root.
        roots = nodes.iter().map(|n| n.name.as_str()).collect();
        roots.sort_unstable();
        if let Some(first) = roots.first().copied() {
            roots = vec![first];
        }
    }

    let mut expanded = HashSet::new();
    for root in roots {
        print_tree_node(root, "", true, true, &children, &states, &mut expanded);
    }
}

fn print_tree_node(
    name: &str,
    prefix: &str,
    is_root: bool,
    is_last: bool,
    children: &HashMap<&str, Vec<&str>>,
    states: &HashMap<&str, &DepNode>,
    expanded: &mut HashSet<String>,
) {
    let state = states
        .get(name)
        .map(|n| n.state.to_string())
        .unwrap_or_else(|| "?".into());
    let already = expanded.contains(name);
    let marker = if already { " [already shown]" } else { "" };

    if is_root {
        println!("  {name} ({state}){marker}");
    } else {
        let branch = if is_last { "└─>" } else { "├─>" };
        println!("{prefix}{branch} {name} ({state}){marker}");
    }

    if already {
        return;
    }
    expanded.insert(name.to_string());

    let Some(kids) = children.get(name) else {
        return;
    };
    let child_prefix = if is_root {
        String::from("  ")
    } else if is_last {
        format!("{prefix}    ")
    } else {
        format!("{prefix}│   ")
    };
    for (i, kid) in kids.iter().enumerate() {
        let last = i + 1 == kids.len();
        print_tree_node(kid, &child_prefix, false, last, children, states, expanded);
    }
}

pub fn cmd_start(socket: &Path, name: &str, force: bool) -> Result<()> {
    match request(
        socket,
        &Request::Start {
            name: name.into(),
            force,
        },
    )? {
        Response::Ok { message } => {
            if let Some(msg) = message {
                println!("{msg}");
            }
            Ok(())
        }
        Response::Error { message } => Err(Error::Ipc(message)),
        other => Err(Error::Ipc(format!("unexpected response: {other:?}"))),
    }
}

pub fn cmd_stop(socket: &Path, name: &str) -> Result<()> {
    simple_ok(socket, Request::Stop { name: name.into() })
}

pub fn cmd_restart(socket: &Path, name: &str) -> Result<()> {
    simple_ok(socket, Request::Restart { name: name.into() })
}

pub fn cmd_enable(socket: &Path, name: &str) -> Result<()> {
    simple_ok(
        socket,
        Request::Enable {
            name: name.into(),
            enabled: true,
        },
    )
}

pub fn cmd_disable(socket: &Path, name: &str) -> Result<()> {
    simple_ok(
        socket,
        Request::Enable {
            name: name.into(),
            enabled: false,
        },
    )
}

pub fn cmd_logs(
    socket: &Path,
    name: Option<String>,
    follow: bool,
    lines: Option<usize>,
) -> Result<()> {
    let mut stream = crate::ipc::connect(socket)?;
    write_frame(
        &mut stream,
        &Request::Logs {
            name,
            follow,
            lines,
        },
    )?;

    let stdout = io::stdout();
    let mut out = stdout.lock();
    loop {
        let resp: Response = read_frame(&mut stream)?;
        match resp {
            Response::Log { line } => {
                writeln!(out, "[{}] {}: {}", line.ts, line.service, line.msg)?;
                out.flush()?;
            }
            Response::Ok { .. } => break,
            Response::Error { message } => return Err(Error::Ipc(message)),
            other => return Err(Error::Ipc(format!("unexpected: {other:?}"))),
        }
    }
    Ok(())
}

fn simple_ok(socket: &Path, req: Request) -> Result<()> {
    match request(socket, &req)? {
        Response::Ok { message } => {
            if let Some(msg) = message {
                println!("{msg}");
            }
            Ok(())
        }
        Response::Error { message } => Err(Error::Ipc(message)),
        other => Err(Error::Ipc(format!("unexpected response: {other:?}"))),
    }
}

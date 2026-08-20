//! CLI handlers that talk to the running `microinit init` daemon via IPC.

use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::path::Path;

use crate::error::{Error, Result};
use crate::ipc::{read_frame, request, write_frame};
use crate::protocol::{
    DaemonInfo, DepNode, Request, Response, ServiceDescribe, ServiceEvent, ServiceStatus,
    ShutdownMode,
};

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

/// Show daemon build/runtime info (`microinit info`).
pub fn cmd_info(socket: &Path, json: bool) -> Result<()> {
    match request(socket, &Request::Info)? {
        Response::Info { info } => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&info)
                        .map_err(|e| { Error::Ipc(format!("serialize info: {e}")) })?
                );
            } else {
                print_daemon_info(&info);
            }
            Ok(())
        }
        Response::Error { message, .. } => Err(Error::Ipc(message)),
        other => Err(Error::Ipc(format!("unexpected response: {other:?}"))),
    }
}

fn print_daemon_info(info: &DaemonInfo) {
    let commit = short_commit(&info.tag_commit);
    let build = short_commit(&info.build_commit);
    println!("Version:     {}", info.version);
    println!(
        "Tag commit:  {}",
        if commit.is_empty() { "-" } else { &commit }
    );
    print!("Build:       {build}");
    if !info.build_time.is_empty() {
        print!(" {}", info.build_time);
    }
    println!();
    println!("Mode:        {}", info.mode);
    println!("PID:         {}", info.pid);
    println!("Hostname:    {}", info.hostname);
    println!("Uptime:      {}", format_uptime(Some(info.uptime_secs)));
    println!("Socket:      {}", info.socket);
    println!(
        "Services:    {} total, {} running",
        info.services_total, info.services_running
    );
    println!("OpenTelemetry:");
    println!(
        "  Enabled:   {}",
        if info.otel_enabled { "yes" } else { "no" }
    );
    println!("  Endpoint:  {}", info.otel_endpoint);
    println!("  Protocol:  {}", info.otel_protocol);
    println!("  Interval:  {}s", info.otel_export_interval_secs);
    println!("  Service:   {}", info.otel_service_name);
}

fn short_commit(full: &str) -> String {
    full.chars().take(7).collect()
}

pub fn cmd_list(socket: &Path, show_labels: bool, selectors: &[String]) -> Result<()> {
    let want: Vec<(String, String)> = selectors
        .iter()
        .map(|s| crate::labels::parse_selector(s))
        .collect::<Result<Vec<_>>>()?;
    match request(socket, &Request::List)? {
        Response::List { services } => {
            let services: Vec<_> = services
                .into_iter()
                .filter(|s| crate::labels::matches_selectors(&s.labels, &want))
                .collect();
            if show_labels {
                println!(
                    "{:<20} {:<22} {:>8} {:>8} {:>8} {:>10} LABELS",
                    "NAME", "STATE", "PID", "RESTARTS", "ENABLED", "LIVE_FAIL"
                );
            } else {
                println!(
                    "{:<20} {:<22} {:>8} {:>8} {:>8} {:>10}",
                    "NAME", "STATE", "PID", "RESTARTS", "ENABLED", "LIVE_FAIL"
                );
            }
            for s in services {
                let pid = s.pid.map(|p| p.to_string()).unwrap_or_else(|| "-".into());
                if show_labels {
                    let labels = crate::labels::format_labels(&s.labels);
                    println!(
                        "{:<20} {:<22} {:>8} {:>8} {:>8} {:>10} {}",
                        s.name,
                        s.state.to_string(),
                        pid,
                        s.restarts,
                        if s.enabled { "yes" } else { "no" },
                        s.liveness_failures,
                        if labels.is_empty() { "-" } else { &labels }
                    );
                } else {
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
            }
            Ok(())
        }
        Response::Error { message, .. } => Err(Error::Ipc(message)),
        other => Err(Error::Ipc(format!("unexpected response: {other:?}"))),
    }
}

pub fn cmd_describe(
    socket: &Path,
    name: &str,
    output: crate::protocol::DescribeOutput,
) -> Result<()> {
    match request(
        socket,
        &Request::Describe {
            name: name.into(),
            output,
        },
    )? {
        Response::Describe { describe } => {
            match output {
                crate::protocol::DescribeOutput::Json => print_describe_json(&describe)?,
                crate::protocol::DescribeOutput::Human => print_describe(&describe),
            }
            Ok(())
        }
        Response::Error { message, .. } => Err(Error::Ipc(message)),
        other => Err(Error::Ipc(format!("unexpected response: {other:?}"))),
    }
}

fn print_describe_json(d: &ServiceDescribe) -> Result<()> {
    let Some(ref src) = d.source else {
        return Err(Error::Ipc(
            "describe -o json: daemon did not return source".into(),
        ));
    };
    eprintln!("# from: {}", src.path);
    println!("{}", serde_json::to_string_pretty(&src.json)?);
    Ok(())
}

fn print_describe(d: &ServiceDescribe) {
    let s = &d.status;
    let pid = s.pid.map(|p| p.to_string()).unwrap_or_else(|| "-".into());
    println!("Service: {}", s.name);
    println!("State:   {}", s.state);
    println!("PID:     {pid}");
    println!("Running as: {}", format_running_as(d.running_as.as_ref()));
    println!(
        "Security:  {}",
        format_security_context(d.security_context.as_ref())
    );
    println!("Enabled: {}", if s.enabled { "yes" } else { "no" });
    println!("Restarts: {}", s.restarts);
    println!("Liveness failures: {}", s.liveness_failures);
    println!("Uptime:  {}", format_uptime(d.uptime_secs));
    println!();

    println!("Labels:");
    if s.labels.is_empty() {
        println!("  (none)");
    } else {
        for (k, v) in &s.labels {
            println!("  {k}={v}");
        }
    }
    println!();

    println!("Depends on:");
    print_dep_list(&d.depends_on);
    println!();

    println!("Required by:");
    print_dep_list(&d.dependents);
    println!();

    println!("Dependency graph:");
    print!("{}", format_dep_graph(&d.dep_nodes, &d.dep_edges));
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

fn format_running_as(id: Option<&crate::protocol::RunningIdentity>) -> String {
    let Some(id) = id else {
        return "-".into();
    };
    let user = id.user.as_deref().unwrap_or("?");
    let group = id.group.as_deref().unwrap_or("?");
    format!("{user}({}) / {group}({})", id.uid, id.gid)
}

fn format_security_context(sec: Option<&crate::config::SecurityContext>) -> String {
    let Some(sec) = sec else {
        return "(none)".into();
    };
    let user = sec.run_as_user.as_deref().unwrap_or("-");
    let group = sec.run_as_group.as_deref().unwrap_or("-");
    let caps = if sec.capabilities.is_empty() {
        "[]".into()
    } else {
        format!("[{}]", sec.capabilities.join(", "))
    };
    format!("runAsUser={user} runAsGroup={group} capabilities={caps}")
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
///
/// Returns a multi-line string (each line ends with `\n`) so unit tests can
/// assert on the rendered graph without capturing stdout.
fn format_dep_graph(nodes: &[DepNode], edges: &[(String, String)]) -> String {
    let mut out = String::new();
    // BFS in `describe` only visits nodes reachable via edges, so an empty
    // edge set means an isolated service (at most the subject itself).
    if edges.is_empty() {
        out.push_str("  (none)\n");
        return out;
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
    let mut ctx = TreeCtx {
        children: &children,
        states: &states,
        expanded: &mut expanded,
    };
    for root in roots {
        write_tree_node(&mut out, root, "", true, true, &mut ctx);
    }
    out
}

struct TreeCtx<'a> {
    children: &'a HashMap<&'a str, Vec<&'a str>>,
    states: &'a HashMap<&'a str, &'a DepNode>,
    expanded: &'a mut HashSet<String>,
}

fn write_tree_node(
    out: &mut String,
    name: &str,
    prefix: &str,
    is_root: bool,
    is_last: bool,
    ctx: &mut TreeCtx<'_>,
) {
    let state = ctx
        .states
        .get(name)
        .map(|n| n.state.to_string())
        .unwrap_or_else(|| "?".into());
    let already = ctx.expanded.contains(name);
    let marker = if already { " [already shown]" } else { "" };

    if is_root {
        out.push_str(&format!("  {name} ({state}){marker}\n"));
    } else {
        let branch = if is_last { "└─>" } else { "├─>" };
        out.push_str(&format!("{prefix}{branch} {name} ({state}){marker}\n"));
    }

    if already {
        return;
    }
    ctx.expanded.insert(name.to_string());

    let Some(kids) = ctx.children.get(name) else {
        return;
    };
    // Clone kids so we can re-borrow ctx mutably in the loop.
    let kids: Vec<&str> = kids.to_vec();
    let child_prefix = if is_root {
        String::from("  ")
    } else if is_last {
        format!("{prefix}    ")
    } else {
        format!("{prefix}│   ")
    };
    for (i, kid) in kids.iter().enumerate() {
        let last = i + 1 == kids.len();
        write_tree_node(out, kid, &child_prefix, false, last, ctx);
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
        Response::Error { message, .. } => Err(Error::Ipc(message)),
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
            Response::Heartbeat => {}
            Response::Ok { .. } => break,
            Response::Error { message, .. } => return Err(Error::Ipc(message)),
            other => return Err(Error::Ipc(format!("unexpected: {other:?}"))),
        }
    }
    Ok(())
}

/// Stream coalesced `list` snapshots (`microinit watch`).
pub fn cmd_watch(socket: &Path, label_keys: &[String], json: bool) -> Result<()> {
    let mut stream = crate::ipc::connect(socket)?;
    write_frame(
        &mut stream,
        &Request::Watch {
            label_keys: label_keys.to_vec(),
        },
    )?;

    let stdout = io::stdout();
    let mut out = stdout.lock();
    loop {
        let resp: Response = read_frame(&mut stream)?;
        match resp {
            Response::List { services } => {
                if json {
                    writeln!(
                        out,
                        "{}",
                        serde_json::to_string(&services)
                            .map_err(|e| Error::Ipc(format!("serialize watch: {e}")))?
                    )?;
                } else {
                    print_list_table(&mut out, &services)?;
                    writeln!(out)?;
                }
                out.flush()?;
            }
            Response::Heartbeat => {}
            Response::Error { message, .. } => return Err(Error::Ipc(message)),
            other => return Err(Error::Ipc(format!("unexpected: {other:?}"))),
        }
    }
}

fn print_list_table(out: &mut impl Write, services: &[ServiceStatus]) -> Result<()> {
    writeln!(
        out,
        "{:<20} {:<22} {:>8} {:>8} {:>8} {:>10}",
        "NAME", "STATE", "PID", "RESTARTS", "ENABLED", "LIVE_FAIL"
    )?;
    for s in services {
        let pid = s.pid.map(|p| p.to_string()).unwrap_or_else(|| "-".into());
        writeln!(
            out,
            "{:<20} {:<22} {:>8} {:>8} {:>8} {:>10}",
            s.name,
            s.state.to_string(),
            pid,
            s.restarts,
            if s.enabled { "yes" } else { "no" },
            s.liveness_failures
        )?;
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
        Response::Error { message, .. } => Err(Error::Ipc(message)),
        other => Err(Error::Ipc(format!("unexpected response: {other:?}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::ServiceState;

    fn node(name: &str, state: ServiceState) -> DepNode {
        DepNode {
            name: name.into(),
            state,
        }
    }

    #[test]
    fn format_dep_graph_empty_edges() {
        let nodes = vec![node("solo", ServiceState::Running)];
        assert_eq!(format_dep_graph(&nodes, &[]), "  (none)\n");
        assert_eq!(format_dep_graph(&[], &[]), "  (none)\n");
    }

    #[test]
    fn format_dep_graph_chain() {
        let nodes = vec![
            node("a", ServiceState::Succeeded),
            node("b", ServiceState::Running),
            node("c", ServiceState::Pending),
        ];
        let edges = vec![("a".into(), "b".into()), ("b".into(), "c".into())];
        let got = format_dep_graph(&nodes, &edges);
        assert_eq!(
            got,
            "  a (succeeded)\n  └─> b (running)\n      └─> c (pending)\n"
        );
    }

    #[test]
    fn format_dep_graph_diamond_marks_already_shown() {
        // a → b, a → c, b → d, c → d
        let nodes = vec![
            node("a", ServiceState::Running),
            node("b", ServiceState::Running),
            node("c", ServiceState::Running),
            node("d", ServiceState::Stopped),
        ];
        let edges = vec![
            ("a".into(), "b".into()),
            ("a".into(), "c".into()),
            ("b".into(), "d".into()),
            ("c".into(), "d".into()),
        ];
        let got = format_dep_graph(&nodes, &edges);
        assert!(got.contains("d (stopped)"), "got:\n{got}");
        assert!(
            got.contains("[already shown]"),
            "diamond should mark repeated d, got:\n{got}"
        );
    }

    #[test]
    fn format_dep_graph_full_cycle_picks_lex_smallest_root() {
        let nodes = vec![
            node("b", ServiceState::Running),
            node("a", ServiceState::Running),
        ];
        let edges = vec![("a".into(), "b".into()), ("b".into(), "a".into())];
        let got = format_dep_graph(&nodes, &edges);
        // Lex-smallest root is "a".
        assert!(got.starts_with("  a (running)\n"), "got:\n{got}");
        assert!(got.contains("[already shown]"), "got:\n{got}");
    }
}

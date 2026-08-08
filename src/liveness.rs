//! Liveness probe execution: shell cmd, TCP connect, or HTTP request.

use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

use crate::config::{LivenessProbe, ServiceConfig};
use crate::service::run_shell_quiet_timeout;

/// Result of one probe attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeResult {
    Ok,
    Fail(String),
}

impl ProbeResult {
    #[must_use]
    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Ok)
    }
}

enum ProbeKind<'a> {
    Cmd(&'a str),
    Tcp(&'a str),
    Http(&'a str),
}

fn probe_kind(probe: &LivenessProbe) -> Option<ProbeKind<'_>> {
    let cmd = probe
        .cmd
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let tcp = probe
        .tcp_addr
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let http = probe
        .http_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    match (cmd, tcp, http) {
        (Some(c), None, None) => Some(ProbeKind::Cmd(c)),
        (None, Some(t), None) => Some(ProbeKind::Tcp(t)),
        (None, None, Some(h)) => Some(ProbeKind::Http(h)),
        _ => None,
    }
}

/// Run the configured probe once, honouring `timeout`.
pub fn run_probe(probe: &LivenessProbe, cfg: &ServiceConfig) -> ProbeResult {
    let timeout = Duration::from_secs(probe.timeout.max(1));
    match probe_kind(probe) {
        Some(ProbeKind::Cmd(cmd)) => match run_shell_quiet_timeout(cmd, cfg, timeout) {
            Ok(Some(code)) if probe.is_success(code) => ProbeResult::Ok,
            Ok(Some(code)) => ProbeResult::Fail(format!("exit {code}")),
            Ok(None) => ProbeResult::Fail("timeout".into()),
            Err(e) => ProbeResult::Fail(e.to_string()),
        },
        Some(ProbeKind::Tcp(addr)) => run_tcp_probe(addr, timeout),
        Some(ProbeKind::Http(url)) => {
            run_http_probe(url, &probe.http_method, &probe.http_accepted_codes, timeout)
        }
        None => ProbeResult::Fail("no probe kind configured".into()),
    }
}

fn run_tcp_probe(addr: &str, timeout: Duration) -> ProbeResult {
    let addrs: Vec<SocketAddr> = match addr.to_socket_addrs() {
        Ok(iter) => iter.collect(),
        Err(e) => return ProbeResult::Fail(format!("resolve {addr}: {e}")),
    };
    if addrs.is_empty() {
        return ProbeResult::Fail(format!("resolve {addr}: no addresses"));
    }

    let mut last_err = String::new();
    for sa in addrs {
        match TcpStream::connect_timeout(&sa, timeout) {
            Ok(stream) => {
                let _ = stream.shutdown(std::net::Shutdown::Both);
                return ProbeResult::Ok;
            }
            Err(e) => last_err = e.to_string(),
        }
    }
    ProbeResult::Fail(format!("tcp {addr}: {last_err}"))
}

fn run_http_probe(url: &str, method: &str, accepted: &[u16], timeout: Duration) -> ProbeResult {
    let method = method.trim();
    if method.is_empty() {
        return ProbeResult::Fail("httpMethod empty".into());
    }

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(timeout)
        .timeout_read(timeout)
        .timeout_write(timeout)
        .build();

    match agent.request(method, url).call() {
        Ok(resp) => {
            let code = resp.status();
            let _ = resp.into_string();
            if accepted.contains(&code) {
                ProbeResult::Ok
            } else {
                ProbeResult::Fail(format!("HTTP {code}"))
            }
        }
        Err(ureq::Error::Status(code, resp)) => {
            let _ = resp.into_string();
            if accepted.contains(&code) {
                ProbeResult::Ok
            } else {
                ProbeResult::Fail(format!("HTTP {code}"))
            }
        }
        Err(e) => ProbeResult::Fail(format!("http: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, HashMap};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    use crate::config::RestartPolicy;

    fn cfg() -> ServiceConfig {
        ServiceConfig {
            name: "t".into(),
            enabled: true,
            daemon: false,
            restart_policy: RestartPolicy::None,
            restart_backoff: 1,
            success_exit_codes: vec![0],
            start_wait_secs: 0,
            shutdown_wait_secs: 5,
            background: false,
            order_priority: 100,
            depends_on: vec![],
            cmd: None,
            start_cmd: Some("true".into()),
            stop_cmd: None,
            restart_cmd: None,
            env: HashMap::new(),
            cwd: "/".into(),
            liveness_probe: None,
            labels: BTreeMap::new(),
            security_context: None,
            #[cfg(not(target_os = "android"))]
            resolved_security: None,
        }
    }

    fn listen() -> (TcpListener, String) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        (listener, format!("127.0.0.1:{port}"))
    }

    fn base_probe() -> LivenessProbe {
        LivenessProbe {
            cmd: None,
            http_url: None,
            tcp_addr: None,
            success_exit_codes: vec![0],
            http_accepted_codes: vec![200],
            http_method: "GET".into(),
            interval: 1,
            timeout: 2,
        }
    }

    #[test]
    fn tcp_open_port_ok() {
        let (listener, addr) = listen();
        thread::spawn(move || {
            let _ = listener.accept();
        });
        let mut probe = base_probe();
        probe.tcp_addr = Some(addr);
        assert!(run_probe(&probe, &cfg()).is_ok());
    }

    #[test]
    fn tcp_closed_port_fails() {
        let mut probe = base_probe();
        probe.tcp_addr = Some("127.0.0.1:1".into());
        probe.timeout = 1;
        assert!(!run_probe(&probe, &cfg()).is_ok());
    }

    #[test]
    fn http_ok_and_bad_status() {
        let (listener, addr) = listen();
        thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let mut buf = [0u8; 512];
                let _ = s.read(&mut buf);
                let _ = s.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            }
        });
        let mut probe = base_probe();
        probe.http_url = Some(format!("http://{addr}/"));
        assert!(run_probe(&probe, &cfg()).is_ok());

        let (listener2, addr2) = listen();
        thread::spawn(move || {
            if let Ok((mut s, _)) = listener2.accept() {
                let mut buf = [0u8; 512];
                let _ = s.read(&mut buf);
                let _ = s.write_all(
                    b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            }
        });
        let mut probe_bad = base_probe();
        probe_bad.http_url = Some(format!("http://{addr2}/"));
        match run_probe(&probe_bad, &cfg()) {
            ProbeResult::Fail(msg) => assert!(msg.contains("503"), "{msg}"),
            ProbeResult::Ok => panic!("expected fail"),
        }
    }

    #[test]
    fn cmd_timeout() {
        let mut probe = base_probe();
        probe.cmd = Some("sleep 30".into());
        probe.timeout = 1;
        match run_probe(&probe, &cfg()) {
            ProbeResult::Fail(msg) => assert_eq!(msg, "timeout"),
            ProbeResult::Ok => panic!("expected timeout"),
        }
    }

    #[test]
    fn http_accepted_non_2xx() {
        let (listener, addr) = listen();
        thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let mut buf = [0u8; 512];
                let _ = s.read(&mut buf);
                let _ = s.write_all(
                    b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            }
        });
        let mut probe = base_probe();
        probe.http_url = Some(format!("http://{addr}/"));
        probe.http_accepted_codes = vec![204];
        assert!(run_probe(&probe, &cfg()).is_ok());
    }
}

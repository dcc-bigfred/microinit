//! Tests for SysV-style shutdown CLI argument parsing.

use microinit::cli::{parse_shutdown_args, ShutdownCliMode};
use microinit::protocol::ShutdownMode;

#[test]
fn parse_shutdown_args_defaults_and_flags() {
    let cases: &[(&[&str], ShutdownCliMode)] = &[
        (&[], ShutdownCliMode::Mode(ShutdownMode::Poweroff)),
        (&["now"], ShutdownCliMode::Mode(ShutdownMode::Poweroff)),
        (&["-h"], ShutdownCliMode::Mode(ShutdownMode::Poweroff)),
        (&["-P", "now"], ShutdownCliMode::Mode(ShutdownMode::Poweroff)),
        (&["-r"], ShutdownCliMode::Mode(ShutdownMode::Reboot)),
        (&["reboot"], ShutdownCliMode::Mode(ShutdownMode::Reboot)),
        (&["-H"], ShutdownCliMode::Mode(ShutdownMode::Halt)),
        (&["--help"], ShutdownCliMode::Help),
    ];
    for (args, want) in cases {
        let got = parse_shutdown_args(args).expect("parse");
        assert_eq!(got, *want, "args={args:?}");
    }
}

#[test]
fn parse_shutdown_args_rejects_conflicts_and_unknown() {
    assert!(parse_shutdown_args(&["-r", "-h"]).is_err());
    assert!(parse_shutdown_args(&["-x"]).is_err());
}

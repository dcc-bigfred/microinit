package config

import "strings"

// ShellQuote wraps value in single quotes, escaping any embedded single
// quotes so the result is safe to interpolate into a POSIX shell command
// line. Use it for every argv token assembled into ServiceDef.StartCmd.
func ShellQuote(value string) string {
	return "'" + strings.ReplaceAll(value, "'", "'\\''") + "'"
}

// BuildStartCmd joins args into a single shell command line, shell-quoting
// each token and prefixing with "exec " so microinit's spawn shell replaces
// itself with the daemon (no extra shell PID lingering).
func BuildStartCmd(args []string) string {
	quoted := make([]string, len(args))
	for i, a := range args {
		quoted[i] = ShellQuote(a)
	}
	return "exec " + strings.Join(quoted, " ")
}

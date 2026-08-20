package client

import (
	"encoding/binary"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"strings"
	"time"
)

const (
	// DefaultSocket is the hub control socket when DATA_DIR is /data.
	DefaultSocket  = "/data/run/microinit.sock"
	maxFrameBytes  = 16 * 1024 * 1024
	defaultTimeout = 10 * time.Second
)

var (
	ErrInvalidName   = errors.New("invalid service name")
	ErrInvalidAction = errors.New("invalid action")
	ErrNotFound      = errors.New("service not found")
)

// ServiceStatus mirrors microinit IPC list/status entries.
type ServiceStatus struct {
	Name             string            `json:"name"`
	State            string            `json:"state"`
	PID              *int32            `json:"pid"`
	Restarts         uint32            `json:"restarts"`
	Enabled          bool              `json:"enabled"`
	LivenessFailures uint32            `json:"liveness_failures,omitempty"`
	Labels           map[string]string `json:"labels,omitempty"`
}

// DaemonInfo mirrors microinit IPC `info` response.
type DaemonInfo struct {
	Version                string `json:"version"`
	TagCommit              string `json:"tag_commit"`
	BuildCommit            string `json:"build_commit"`
	BuildTime              string `json:"build_time"`
	PID                    uint32 `json:"pid"`
	Hostname               string `json:"hostname"`
	UptimeSecs             uint64 `json:"uptime_secs"`
	Socket                 string `json:"socket"`
	Mode                   string `json:"mode"` // "init" | "supervise"
	ServicesTotal          int    `json:"services_total"`
	ServicesRunning        int    `json:"services_running"`
	OtelEnabled            bool   `json:"otel_enabled"`
	OtelEndpoint           string `json:"otel_endpoint"`
	OtelProtocol           string `json:"otel_protocol"`
	OtelServiceName        string `json:"otel_service_name"`
	OtelExportIntervalSecs uint64 `json:"otel_export_interval_secs"`
}

// LogLine is one captured log line from microinit.
type LogLine struct {
	TS      string `json:"ts"`
	Service string `json:"service"`
	Level   string `json:"level"`
	Msg     string `json:"msg"`
}

type request struct {
	Type      string   `json:"type"`
	Name      string   `json:"name,omitempty"`
	Follow    *bool    `json:"follow,omitempty"`
	Lines     *uint64  `json:"lines,omitempty"`
	Mode      string   `json:"mode,omitempty"`
	LabelKeys []string `json:"label_keys,omitempty"`
}

// Response is one framed IPC reply (exported for streaming callers).
type Response struct {
	Type     string          `json:"type"`
	Message  string          `json:"message,omitempty"`
	Code     string          `json:"code,omitempty"`
	Services []ServiceStatus `json:"services,omitempty"`
	Status   *ServiceStatus  `json:"status,omitempty"`
	Info     *DaemonInfo     `json:"info,omitempty"`
	Line     *LogLine        `json:"line,omitempty"`
}

// Client dials the microinit Unix socket.
type Client struct {
	Socket  string
	Timeout time.Duration
	// ReadTimeout is the per-frame idle timeout for streaming reads
	// (FollowLogs). Zero defaults to 30s. Use a larger value on slow
	// embedded links; use a smaller one to detect a dead server faster.
	ReadTimeout time.Duration
	// Dial is overridden in tests.
	Dial func(network, address string, timeout time.Duration) (net.Conn, error)
}

// defaultReadTimeout is the per-frame idle deadline for FollowLogs streams.
const defaultReadTimeout = 30 * time.Second

func (c *Client) readTimeout() time.Duration {
	if c.ReadTimeout > 0 {
		return c.ReadTimeout
	}
	return defaultReadTimeout
}

func (c *Client) socketPath() string {
	if c.Socket != "" {
		return c.Socket
	}
	return DefaultSocket
}

func (c *Client) timeout() time.Duration {
	if c.Timeout > 0 {
		return c.Timeout
	}
	return defaultTimeout
}

func (c *Client) dial() (net.Conn, error) {
	dial := c.Dial
	if dial == nil {
		dial = net.DialTimeout
	}
	conn, err := dial("unix", c.socketPath(), c.timeout())
	if err != nil {
		return nil, fmt.Errorf("connect %s: %w (is microinit running?)", c.socketPath(), err)
	}
	_ = conn.SetDeadline(time.Now().Add(c.timeout()))
	return conn, nil
}

// List returns all services known to microinit.
func (c *Client) List() ([]ServiceStatus, error) {
	var resp Response
	if err := c.roundTrip(request{Type: "list"}, &resp); err != nil {
		return nil, err
	}
	switch resp.Type {
	case "list":
		if resp.Services == nil {
			return []ServiceStatus{}, nil
		}
		return resp.Services, nil
	case "error":
		return nil, responseError(resp)
	default:
		return nil, fmt.Errorf("unexpected response type %q", resp.Type)
	}
}

// Info returns the daemon build/runtime snapshot (`microinit info`).
func (c *Client) Info() (*DaemonInfo, error) {
	var resp Response
	if err := c.roundTrip(request{Type: "info"}, &resp); err != nil {
		return nil, err
	}
	switch resp.Type {
	case "info":
		if resp.Info == nil {
			return nil, fmt.Errorf("info response missing payload")
		}
		return resp.Info, nil
	case "error":
		return nil, responseError(resp)
	default:
		return nil, fmt.Errorf("unexpected response type %q", resp.Type)
	}
}

// Status returns detailed status for one service.
func (c *Client) Status(name string) (*ServiceStatus, error) {
	if err := ValidateName(name); err != nil {
		return nil, err
	}
	var resp Response
	if err := c.roundTrip(request{Type: "status", Name: name}, &resp); err != nil {
		return nil, err
	}
	if resp.Type == "error" {
		return nil, responseError(resp)
	}
	if resp.Type != "status" || resp.Status == nil {
		return nil, fmt.Errorf("unexpected response type %q", resp.Type)
	}
	return resp.Status, nil
}

// Control runs start|stop|restart for a service.
func (c *Client) Control(name, action string) error {
	if err := ValidateName(name); err != nil {
		return err
	}
	switch action {
	case "start", "stop", "restart":
	default:
		return ErrInvalidAction
	}
	var resp Response
	if err := c.roundTrip(request{Type: action, Name: name}, &resp); err != nil {
		return err
	}
	switch resp.Type {
	case "ok":
		return nil
	case "error":
		return responseError(resp)
	default:
		return fmt.Errorf("unexpected response type %q", resp.Type)
	}
}

// Shutdown requests a halt-mode shutdown. Prefer [Client.ShutdownMode] when
// you need reboot/poweroff (e.g. host power control). Halt is for stopping a
// supervise instance started by the caller (CLI / embedder); BigFred's admin
// HTTP API does not expose halt to the UI.
func (c *Client) Shutdown() error {
	return c.ShutdownMode("halt")
}

// ShutdownMode requests a machine/process shutdown with mode reboot|poweroff|halt.
// In supervise mode microinit ignores the machine power aspect and exits after
// stopping services; in init mode it finalizes via reboot(2).
//
// Modes:
//   - reboot / poweroff — host power (init mode); used by BigFred admin UI
//   - halt — process exit after stopping services; for direct SDK/CLI use only
func (c *Client) ShutdownMode(mode string) error {
	switch mode {
	case "reboot", "poweroff", "halt":
	default:
		return fmt.Errorf("invalid shutdown mode %q (want reboot|poweroff|halt)", mode)
	}
	var resp Response
	if err := c.roundTrip(request{Type: "shutdown", Mode: mode}, &resp); err != nil {
		return err
	}
	switch resp.Type {
	case "ok":
		return nil
	case "error":
		return responseError(resp)
	default:
		return fmt.Errorf("unexpected response type %q", resp.Type)
	}
}

// FollowLogs opens a streaming connection. Caller must Close the conn.
//
// lines < 0 uses the server default buffer size; lines >= 0 requests exactly
// that many historical lines (0 = live-only, no snapshot).
//
// Prefer [Client.ReadFrame] for follow streams: it enforces a per-frame idle
// deadline (default 30s) and skips server heartbeats so a quiet but live
// service does not time out.
func (c *Client) FollowLogs(name string, lines int, follow bool) (net.Conn, error) {
	if name != "" {
		if err := ValidateName(name); err != nil {
			return nil, err
		}
	}
	conn, err := c.dial()
	if err != nil {
		return nil, err
	}
	f := follow
	req := request{Type: "logs", Follow: &f}
	if lines >= 0 {
		n := uint64(lines)
		req.Lines = &n
	}
	if name != "" {
		req.Name = name
	}
	if err := writeFrame(conn, req); err != nil {
		_ = conn.Close()
		return nil, err
	}
	_ = conn.SetDeadline(time.Time{})
	return conn, nil
}

// Watch opens a streaming connection that delivers coalesced `list`
// snapshots. Caller must Close the conn.
//
// labelKeys, when non-empty, keep only services that possess every listed
// key (presence; any value). An empty/nil slice watches all services.
//
// Use [Client.ReadFrame] to consume the stream: it skips heartbeats and
// enforces a per-frame idle deadline. Each non-heartbeat frame is a `list`
// response (`Response.Services`).
func (c *Client) Watch(labelKeys []string) (net.Conn, error) {
	conn, err := c.dial()
	if err != nil {
		return nil, err
	}
	req := request{Type: "watch"}
	if len(labelKeys) > 0 {
		req.LabelKeys = labelKeys
	}
	if err := writeFrame(conn, req); err != nil {
		_ = conn.Close()
		return nil, err
	}
	_ = conn.SetDeadline(time.Time{})
	return conn, nil
}

// ReadResponse reads one framed response from a FollowLogs connection.
// It does not enforce a read deadline; callers wanting idle-timeout
// protection should use [Client.ReadFrame] instead, or set the deadline
// on the conn themselves before each call.
func ReadResponse(r io.Reader) (Response, error) {
	var resp Response
	if err := readFrame(r, &resp); err != nil {
		return Response{}, err
	}
	return resp, nil
}

// ReadFrame reads one framed response from a streaming connection and
// enforces a per-frame idle read deadline (Client.ReadTimeout, default 30s).
// Server heartbeats (type "heartbeat") are skipped so a quiet-but-live
// follow stream stays open; a dead server is detected via deadline or EOF.
func (c *Client) ReadFrame(conn net.Conn) (Response, error) {
	for {
		_ = conn.SetReadDeadline(time.Now().Add(c.readTimeout()))
		var resp Response
		if err := readFrame(conn, &resp); err != nil {
			return Response{}, err
		}
		if resp.Type == "heartbeat" {
			continue
		}
		return resp, nil
	}
}

// FormatLogLine renders a LogLine for console / UI output.
func FormatLogLine(line LogLine) string {
	if line.TS == "" {
		return fmt.Sprintf("%s: %s", line.Service, line.Msg)
	}
	return fmt.Sprintf("[%s] %s: %s", line.TS, line.Service, line.Msg)
}

// ValidateName reports whether name is a safe microinit service id.
func ValidateName(name string) error {
	if name == "" || strings.ContainsAny(name, `/\`) || strings.Contains(name, "..") {
		return ErrInvalidName
	}
	for _, r := range name {
		if (r >= 'a' && r <= 'z') || (r >= 'A' && r <= 'Z') || (r >= '0' && r <= '9') ||
			r == '-' || r == '_' || r == '.' {
			continue
		}
		return ErrInvalidName
	}
	return nil
}

func (c *Client) roundTrip(req request, resp *Response) error {
	conn, err := c.dial()
	if err != nil {
		return err
	}
	defer conn.Close()
	if err := writeFrame(conn, req); err != nil {
		return err
	}
	return readFrame(conn, resp)
}

func writeFrame(w io.Writer, msg any) error {
	payload, err := json.Marshal(msg)
	if err != nil {
		return err
	}
	if len(payload) > maxFrameBytes {
		return errors.New("frame too large")
	}
	var hdr [4]byte
	binary.LittleEndian.PutUint32(hdr[:], uint32(len(payload)))
	if err := writeFull(w, hdr[:]); err != nil {
		return err
	}
	return writeFull(w, payload)
}

func writeFull(w io.Writer, p []byte) error {
	for len(p) > 0 {
		n, err := w.Write(p)
		if n > 0 {
			p = p[n:]
		}
		if err != nil {
			return err
		}
		if n == 0 {
			return io.ErrShortWrite
		}
	}
	return nil
}

func readFrame(r io.Reader, dest any) error {
	var hdr [4]byte
	if _, err := io.ReadFull(r, hdr[:]); err != nil {
		return err
	}
	n := binary.LittleEndian.Uint32(hdr[:])
	if n > maxFrameBytes {
		return fmt.Errorf("frame length %d too large", n)
	}
	// Read exactly n bytes so the next frame stays aligned, then decode.
	// Size is capped by maxFrameBytes above.
	buf := make([]byte, n)
	if _, err := io.ReadFull(r, buf); err != nil {
		return err
	}
	return json.Unmarshal(buf, dest)
}

// responseError maps an IPC error response to a typed error. It prefers the
// stable `code` field (populated by microinit for Error::UnknownService etc.)
// and falls back to substring-matching the human message for older servers
// that do not send a code.
func responseError(resp Response) error {
	switch resp.Code {
	case "not_found":
		return ErrNotFound
	case "disabled":
		return fmt.Errorf("%s: %w", resp.Message, ErrNotFound)
	case "":
		// Legacy server without a code field.
		lower := strings.ToLower(resp.Message)
		if strings.Contains(lower, "unknown") || strings.Contains(lower, "not found") {
			return ErrNotFound
		}
	}
	if resp.Message == "" {
		return errors.New("microinit request failed")
	}
	return errors.New(resp.Message)
}

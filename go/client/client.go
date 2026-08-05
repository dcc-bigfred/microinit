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

// LogLine is one captured log line from microinit.
type LogLine struct {
	TS      string `json:"ts"`
	Service string `json:"service"`
	Level   string `json:"level"`
	Msg     string `json:"msg"`
}

type request struct {
	Type   string  `json:"type"`
	Name   string  `json:"name,omitempty"`
	Follow *bool   `json:"follow,omitempty"`
	Lines  *uint64 `json:"lines,omitempty"`
	Mode   string  `json:"mode,omitempty"`
}

// Response is one framed IPC reply (exported for streaming callers).
type Response struct {
	Type     string          `json:"type"`
	Message  string          `json:"message,omitempty"`
	Services []ServiceStatus `json:"services,omitempty"`
	Status   *ServiceStatus  `json:"status,omitempty"`
	Line     *LogLine        `json:"line,omitempty"`
}

// Client dials the microinit Unix socket.
type Client struct {
	Socket  string
	Timeout time.Duration
	// Dial is overridden in tests.
	Dial func(network, address string, timeout time.Duration) (net.Conn, error)
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
		return nil, responseError(resp.Message)
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
		return nil, responseError(resp.Message)
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
		return responseError(resp.Message)
	default:
		return fmt.Errorf("unexpected response type %q", resp.Type)
	}
}

// Shutdown requests a halt-mode shutdown (used when stopping a supervise
// instance started by the caller).
func (c *Client) Shutdown() error {
	var resp Response
	if err := c.roundTrip(request{Type: "shutdown", Mode: "halt"}, &resp); err != nil {
		return err
	}
	switch resp.Type {
	case "ok":
		return nil
	case "error":
		return responseError(resp.Message)
	default:
		return fmt.Errorf("unexpected response type %q", resp.Type)
	}
}

// FollowLogs opens a streaming connection. Caller must Close the conn.
//
// lines < 0 uses the server default buffer size; lines >= 0 requests exactly
// that many historical lines (0 = live-only, no snapshot).
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

// ReadResponse reads one framed response from a FollowLogs connection.
func ReadResponse(r io.Reader) (Response, error) {
	var resp Response
	if err := readFrame(r, &resp); err != nil {
		return Response{}, err
	}
	return resp, nil
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
	if _, err := w.Write(hdr[:]); err != nil {
		return err
	}
	_, err = w.Write(payload)
	return err
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
	buf := make([]byte, n)
	if _, err := io.ReadFull(r, buf); err != nil {
		return err
	}
	return json.Unmarshal(buf, dest)
}

func responseError(message string) error {
	lower := strings.ToLower(message)
	if strings.Contains(lower, "unknown") || strings.Contains(lower, "not found") {
		return ErrNotFound
	}
	if message == "" {
		message = "microinit request failed"
	}
	return errors.New(message)
}

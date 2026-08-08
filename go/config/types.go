package config

// ServiceDef is one service entry in microinit.json or a drop-in file.
type ServiceDef struct {
	Name             string            `json:"name"`
	Enabled          *bool             `json:"enabled,omitempty"`
	Daemon           *bool             `json:"daemon,omitempty"`
	// RestartPolicy is "always", "onError" (default), or "none".
	RestartPolicy    string            `json:"restartPolicy,omitempty"`
	RestartBackoff   *int              `json:"restartBackoff,omitempty"`
	StartWaitSecs    *int              `json:"startWaitSecs,omitempty"`
	ShutdownWaitSecs *int              `json:"shutdownWaitSecs,omitempty"`
	// OrderPriority: among ready services, lower starts earlier (default 100).
	OrderPriority    *int              `json:"orderPriority,omitempty"`
	DependsOn        []string          `json:"dependsOn,omitempty"`
	StartCmd         string            `json:"startCmd,omitempty"`
	StopCmd          string            `json:"stopCmd,omitempty"`
	Cmd              string            `json:"cmd,omitempty"`
	Cwd              string            `json:"cwd,omitempty"`
	LivenessProbe    *LivenessProbe    `json:"livenessProbe,omitempty"`
	Labels           map[string]string `json:"labels,omitempty"`
	SecurityContext  *SecurityContext  `json:"securityContext,omitempty"`
}

// SecurityContext drops privileges and optionally keeps Linux capabilities.
// On Android microinit rejects a configured securityContext at load time.
type SecurityContext struct {
	RunAsUser    string   `json:"runAsUser,omitempty"`
	RunAsGroup   string   `json:"runAsGroup,omitempty"`
	Capabilities []string `json:"capabilities,omitempty"`
}

// Restart policy values for ServiceDef.RestartPolicy.
const (
	RestartAlways  = "always"
	RestartOnError = "onError"
	RestartNone    = "none"
)

// LivenessProbe mirrors microinit JSON probe fields.
type LivenessProbe struct {
	HTTPUrl           string `json:"httpUrl,omitempty"`
	HTTPAcceptedCodes []int  `json:"httpAcceptedCodes,omitempty"`
	TCPAddr           string `json:"tcpAddr,omitempty"`
	Cmd               string `json:"cmd,omitempty"`
	SuccessExitCodes  []int  `json:"successExitCodes,omitempty"`
	Interval          int    `json:"interval,omitempty"`
	Timeout           int    `json:"timeout,omitempty"`
}

// DropinFile is the JSON envelope for files under microinit.d/services/.
type DropinFile struct {
	Services []ServiceDef `json:"services"`
}

// BoolPtr returns a pointer to v (for optional JSON bool fields).
func BoolPtr(v bool) *bool { return &v }

// IntPtr returns a pointer to v (for optional JSON int fields).
func IntPtr(v int) *int { return &v }

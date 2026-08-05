# Go SDK for microinit

Module: `github.com/dcc-bigfred/microinit/go`

| Package | Import | Role |
|---------|--------|------|
| client | `github.com/dcc-bigfred/microinit/go/client` | IPC (list/control/logs) |
| config | `github.com/dcc-bigfred/microinit/go/config` | ServiceDef + drop-ins |
| supervise | `github.com/dcc-bigfred/microinit/go/supervise` | Join/spawn daemon in-process |

Full guide with examples: **[docs/sdk/golang.md](../docs/sdk/golang.md)**. Developer index: **[docs/developer.md](../docs/developer.md)**.

## Install

```bash
go get github.com/dcc-bigfred/microinit/go@go/v0.3.0
```

Tag Go releases as **`go/vX.Y.Z`**. Private: `GOPRIVATE=github.com/dcc-bigfred/*`.

```go
// local monorepo
replace github.com/dcc-bigfred/microinit/go => ../microinit/go
```

## Quick start

```go
import (
	"github.com/dcc-bigfred/microinit/go/client"
	"github.com/dcc-bigfred/microinit/go/config"
	"github.com/dcc-bigfred/microinit/go/supervise"
)

c := &client.Client{Socket: client.DefaultSocket}
list, err := c.List()

svc := config.WithCreatedBy(config.ServiceDef{Name: "worker", StartCmd: "exec worker"}, "my-app")

h := supervise.New(socket, "microinit", configPath, dropinDir)
joined, err := h.EnsureRunning(ctx)
// … app writes drop-ins / stops its services …
err = h.Shutdown(ctx) // no-op if joined
```

Default socket: `/data/run/microinit.sock`.

# Go client for microinit

IPC client library for the microinit control socket (length-prefixed JSON over Unix domain sockets).

## Import

```go
import "github.com/dcc-bigfred/microinit/go/client"

c := &client.Client{Socket: client.DefaultSocket} // or override
list, err := c.List()
```

## Module path / versioning

```
module github.com/dcc-bigfred/microinit/go
```

Tag Go releases as **`go/vX.Y.Z`** (required because the module path ends with `/go`), for example:

```bash
git tag go/v0.1.0
git push origin go/v0.1.0
```

Then consumers:

```bash
go get github.com/dcc-bigfred/microinit/go@go/v0.1.0
```

For local monorepo development:

```go
// go.mod
replace github.com/dcc-bigfred/microinit/go => ../microinit/go
```

Private repos need `GOPRIVATE=github.com/dcc-bigfred/*`.

## API

| Method | Description |
|--------|-------------|
| `List()` | All services |
| `Status(name)` | One service |
| `Control(name, start\|stop\|restart)` | Lifecycle |
| `Shutdown()` | Halt supervise (caller-owned daemon) |
| `FollowLogs` / `ReadResponse` | Log stream |
| `ValidateName` / `FormatLogLine` | Helpers |

Default socket: `/data/run/microinit.sock` (`client.DefaultSocket`).

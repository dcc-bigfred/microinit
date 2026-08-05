# Developer documentation

Index of docs for people changing or integrating **microinit**.

## Guides

| Document | Contents |
|----------|----------|
| [Architecture](architecture.md) | `init` vs `supervise`, reload, OTel, distribution |
| [Control socket API](api.md) | Unix socket framing and JSON messages |
| [Operator guide](operator.md) | Shell usage, JSON config, boot sequence |
| [Documentation index](README.md) | Operator-oriented entry point |

## SDKs

| Document | Language | Contents |
|----------|----------|----------|
| [Go SDK](sdk/golang.md) | Go | `client`, `config`, `supervise` — embed or control microinit |

## Source layout (Go)

```
go/
  go.mod                 # module github.com/dcc-bigfred/microinit/go
  client/                # IPC client
  config/                # ServiceDef + drop-ins
  supervise/             # EnsureRunning / Shutdown host
  README.md
```

Version tags: `go/vX.Y.Z`. See [Go SDK](sdk/golang.md) for import examples.

## Man pages / examples

- `man/man8/microinit.8.mdoc` — CLI
- `man/man5/microinit.json.5.mdoc` — config fields
- [`examples/microinit.json.example`](../examples/microinit.json.example)

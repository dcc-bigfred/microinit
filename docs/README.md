# microinit documentation

Guides and references for running and integrating **microinit**.

| Document | Audience | Contents |
|----------|----------|----------|
| [Operator guide](operator.md) | Linux admins / device operators | Managing services from the shell, writing JSON config, dependencies, boot sequence |
| [Control socket API](api.md) | Integrators / UI / scripts | Unix socket framing, request/response JSON |
| [Architecture](architecture.md) | Developers | Design overview: `init` vs `supervise`, reload, OTel, distribution |
| [Developer index](developer.md) | Contributors / embedders | Doc map + Go SDK pointer |
| [Go SDK](sdk/golang.md) | Go integrators | `client` / `config` / `supervise` with examples |

Also see the man pages in the repository:

- `man/man8/microinit.8.mdoc` — CLI and runtime behaviour  
- `man/man5/microinit.json.5.mdoc` — configuration file fields  

Example config: [`examples/microinit.json.example`](../examples/microinit.json.example).

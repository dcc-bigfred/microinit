// Package supervise embeds a microinit daemon inside a client process.
//
// It only manages process lifecycle (join existing socket or spawn
// `microinit supervise`). Stopping individual services, drop-in ownership, and
// product policies belong in the caller.
package supervise

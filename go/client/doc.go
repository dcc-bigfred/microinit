// Package client talks to a running microinit daemon over its Unix control socket.
//
// Protocol: 4-byte little-endian length prefix + UTF-8 JSON frame (max 16 MiB).
package client

// Package web embeds the compiled Svelte frontend (web/dist/).
// Run `npm run build` inside web/ before building the Go binary.
package web

import "embed"

//go:embed dist
var Files embed.FS

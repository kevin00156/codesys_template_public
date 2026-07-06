//go:build !linux

package shm

import (
	"fmt"
	"runtime"
)

// OpenTrace exists only on Linux (/dev/shm); this stub keeps the bridge
// compiling on development machines, mirroring mapping_other.go.
func OpenTrace() (*TraceRing, error) {
	return nil, fmt.Errorf("shm: /dev/shm segments are only supported on linux, not %s", runtime.GOOS)
}

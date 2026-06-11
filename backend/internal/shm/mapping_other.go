//go:build !linux

package shm

import (
	"fmt"
	"runtime"
)

// OpenRead / Open are only implemented on Linux (mmap of /dev/shm).
// On other platforms they fail at runtime but still compile, so the
// rest of the backend builds and its pure-logic tests run during local
// development on Windows/macOS.

func OpenRead(name string, size int) (*Mapping, error) { return nil, errUnsupported() }
func Open(name string, size int) (*Mapping, error)     { return nil, errUnsupported() }

func errUnsupported() error {
	return fmt.Errorf("shm: /dev/shm mmap is only supported on linux, not %s", runtime.GOOS)
}

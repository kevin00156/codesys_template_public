//go:build linux

package shm

import (
	"fmt"
	"os"

	"golang.org/x/sys/unix"
)

// OpenRead mmaps a read-only view of /dev/shm/<name>.
// Use for plc_data: the PLC is the sole writer; Go only reads.
func OpenRead(name string, size int) (*Mapping, error) {
	return openShm(name, size, os.O_RDONLY, unix.PROT_READ)
}

// Open mmaps a read-write view of /dev/shm/<name>.
// Use for plc_cmd: Go writes commands, PLC reads them.
func Open(name string, size int) (*Mapping, error) {
	return openShm(name, size, os.O_RDWR, unix.PROT_READ|unix.PROT_WRITE)
}

func openShm(name string, size, fileFlag, mmapProt int) (*Mapping, error) {
	path := "/dev/shm/" + name
	f, err := os.OpenFile(path, fileFlag, 0)
	if err != nil {
		return nil, fmt.Errorf("open %s: %w", path, err)
	}
	st, err := f.Stat()
	if err != nil {
		f.Close()
		return nil, fmt.Errorf("stat %s: %w", path, err)
	}
	if st.Size() < int64(size) {
		f.Close()
		return nil, fmt.Errorf("%s: segment %d B, expected at least %d B — PLC layout out of date?",
			path, st.Size(), size)
	}
	data, err := unix.Mmap(int(f.Fd()), 0, size, mmapProt, unix.MAP_SHARED)
	if err != nil {
		f.Close()
		return nil, fmt.Errorf("mmap %s: %w", path, err)
	}
	return &Mapping{file: f, data: data, munmap: unix.Munmap}, nil
}

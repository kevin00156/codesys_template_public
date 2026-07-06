//go:build linux

package shm

import (
	"fmt"
	"os"

	"golang.org/x/sys/unix"
)

// OpenTrace mmaps a read-only view of /dev/shm/plc_trace and validates its
// header. The segment size depends on the daemon's --trace-seconds, so the
// whole file is mapped as-is (the header's capacity is validated against it).
func OpenTrace() (*TraceRing, error) {
	path := "/dev/shm/" + NamePlcTrace
	f, err := os.OpenFile(path, os.O_RDONLY, 0)
	if err != nil {
		return nil, fmt.Errorf("open %s: %w", path, err)
	}
	st, err := f.Stat()
	if err != nil {
		f.Close()
		return nil, fmt.Errorf("stat %s: %w", path, err)
	}
	if st.Size() < int64(SizeTraceHeader) {
		f.Close()
		return nil, fmt.Errorf("%s: segment %d B < header %d B — daemon too old?",
			path, st.Size(), SizeTraceHeader)
	}
	data, err := unix.Mmap(int(f.Fd()), 0, int(st.Size()), unix.PROT_READ, unix.MAP_SHARED)
	if err != nil {
		f.Close()
		return nil, fmt.Errorf("mmap %s: %w", path, err)
	}
	m := &Mapping{file: f, data: data, munmap: unix.Munmap}
	ring, err := NewTraceRing(m)
	if err != nil {
		m.Close()
		return nil, err
	}
	return ring, nil
}

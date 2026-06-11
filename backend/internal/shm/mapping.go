package shm

import (
	"os"
	"unsafe"
)

// Mapping is a view over a shared-memory segment.
//
// On Linux it is an mmap of /dev/shm/<name> — see mapping_linux.go.
// On other platforms OpenRead/Open return an error (mapping_other.go),
// so the binary still compiles for local development and unit tests.
// Tests construct an in-memory Mapping directly with mappingFromBytes.
type Mapping struct {
	file   *os.File
	data   []byte
	munmap func([]byte) error // nil for in-memory mappings
}

// mappingFromBytes wraps an existing slice as a Mapping. The slice must
// stay 8-byte aligned and alive for the lifetime of the Mapping. Used by
// tests to exercise the seqlock without touching /dev/shm.
func mappingFromBytes(b []byte) *Mapping { return &Mapping{data: b} }

// Close releases the segment. Safe on in-memory mappings (no-op for the
// munmap; closes the backing file if any).
func (m *Mapping) Close() error {
	var err error
	if m.munmap != nil {
		err = m.munmap(m.data)
	}
	if m.file != nil {
		if cerr := m.file.Close(); err == nil {
			err = cerr
		}
	}
	return err
}

func (m *Mapping) Ptr() unsafe.Pointer { return unsafe.Pointer(&m.data[0]) }
func (m *Mapping) Size() int           { return len(m.data) }

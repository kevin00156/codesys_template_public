package traceapi

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync/atomic"
	"testing"
	"unsafe"

	"codesys_dev/backend/internal/shm"
)

// fixtureRing builds an in-memory plc_trace segment with n samples and a
// handler whose staleness check points at a real temp file.
func fixtureHandler(t *testing.T, capacity uint32, n uint64) *Handler {
	t.Helper()
	size := shm.SizeTraceHeader + int(capacity)*shm.SizeTraceSample
	buf := make([]byte, size)

	hdr := (*shm.TraceHeader)(unsafe.Pointer(&buf[0]))
	*hdr = shm.TraceHeader{
		Magic:       shm.PlcTraceMagic,
		Version:     shm.PlcTraceVersion,
		SampleSize:  uint32(shm.SizeTraceSample),
		Capacity:    capacity,
		PeriodNs:    2_000_000,
		EpochUnixNs: 1_000,
	}
	for i := uint64(0); i < n; i++ {
		off := shm.SizeTraceHeader + int(i&uint64(capacity-1))*shm.SizeTraceSample
		s := (*shm.TraceSample)(unsafe.Pointer(&buf[off]))
		s.Cycle = i
		s.TMonoNs = i * 2_000_000
		s.Axes[0].ActPos = float64(i)
	}
	atomic.StoreUint64(&hdr.WriteIdx, n)

	path := filepath.Join(t.TempDir(), "plc_trace")
	if err := os.WriteFile(path, []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	return NewWithOpener(path, func() (*shm.TraceRing, error) {
		return shm.NewTraceRingFromBytes(buf)
	})
}

func get(t *testing.T, h *Handler, url string) *httptest.ResponseRecorder {
	t.Helper()
	mux := http.NewServeMux()
	h.Register(mux)
	rec := httptest.NewRecorder()
	mux.ServeHTTP(rec, httptest.NewRequest(http.MethodGet, url, nil))
	return rec
}

func TestMeta(t *testing.T) {
	h := fixtureHandler(t, 16, 5)
	rec := get(t, h, "/api/trace/meta")
	if rec.Code != http.StatusOK {
		t.Fatalf("meta: %d %s", rec.Code, rec.Body)
	}
	var m struct {
		PeriodNs uint64   `json:"periodNs"`
		WriteIdx uint64   `json:"writeIdx"`
		Fields   []string `json:"fields"`
	}
	if err := json.Unmarshal(rec.Body.Bytes(), &m); err != nil {
		t.Fatal(err)
	}
	if m.PeriodNs != 2_000_000 || m.WriteIdx != 5 {
		t.Fatalf("meta: %+v", m)
	}
	if len(m.Fields) != len(fields) || m.Fields[0] != "cycle" || m.Fields[7] != "axis0.actPos" {
		t.Fatalf("fields: %v", m.Fields)
	}
}

func TestDataCursor(t *testing.T) {
	h := fixtureHandler(t, 16, 10)
	rec := get(t, h, "/api/trace/data?since=0&max=4")
	var d struct {
		First   uint64      `json:"first"`
		Next    uint64      `json:"next"`
		Dropped uint64      `json:"dropped"`
		Columns [][]float64 `json:"columns"`
	}
	if err := json.Unmarshal(rec.Body.Bytes(), &d); err != nil {
		t.Fatal(err)
	}
	if d.First != 0 || d.Next != 4 || d.Dropped != 0 {
		t.Fatalf("data: %+v", d)
	}
	if len(d.Columns) != len(fields) || len(d.Columns[0]) != 4 {
		t.Fatalf("columns: %d×%d", len(d.Columns), len(d.Columns[0]))
	}
	// cycle column carries the sample index; axis0.actPos mirrors it.
	if d.Columns[0][3] != 3 || d.Columns[7][3] != 3 {
		t.Fatalf("column content: cycle=%v actPos=%v", d.Columns[0], d.Columns[7])
	}

	// Tail request (since=-1) returns the newest samples.
	rec = get(t, h, "/api/trace/data?since=-1&max=4")
	if err := json.Unmarshal(rec.Body.Bytes(), &d); err != nil {
		t.Fatal(err)
	}
	if d.First != 6 || d.Next != 10 {
		t.Fatalf("tail: %+v", d)
	}
}

func TestExportJSONAndCSV(t *testing.T) {
	h := fixtureHandler(t, 16, 10)

	rec := get(t, h, "/api/trace/export?seconds=1")
	var e struct {
		Format  string      `json:"format"`
		Fields  []string    `json:"fields"`
		Columns [][]float64 `json:"columns"`
	}
	if err := json.Unmarshal(rec.Body.Bytes(), &e); err != nil {
		t.Fatal(err)
	}
	if e.Format != "plc-trace/1" || len(e.Fields) != len(fields) {
		t.Fatalf("export: format=%q fields=%d", e.Format, len(e.Fields))
	}
	if len(e.Columns[0]) != 10 { // 1 s @ 2 ms = 500 > available 10 → all 10
		t.Fatalf("export rows: %d", len(e.Columns[0]))
	}

	rec = get(t, h, "/api/trace/export?seconds=1&format=csv")
	lines := strings.Split(strings.TrimSpace(rec.Body.String()), "\n")
	if len(lines) != 11 { // header + 10 rows
		t.Fatalf("csv lines: %d", len(lines))
	}
	if !strings.HasPrefix(lines[0], "cycle,tMonoNs,") {
		t.Fatalf("csv header: %s", lines[0])
	}
}

func TestMissingSegmentIs503(t *testing.T) {
	h := NewWithOpener(filepath.Join(t.TempDir(), "nonexistent"), shm.OpenTrace)
	rec := get(t, h, "/api/trace/meta")
	if rec.Code != http.StatusServiceUnavailable {
		t.Fatalf("want 503, got %d", rec.Code)
	}
}

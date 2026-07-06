// Package traceapi serves the daemon's plc_trace ring over HTTP for the
// HMI's watch/trace panels (CODESYS trace parity).
//
// Stateless by design: the shm ring *is* the history buffer, so there is no
// drain goroutine and no duplicated storage here — every request reads the
// mapped segment directly, and each client carries its own cursor in the
// query string. Endpoints (read-only, same open-read posture as the WS data
// push):
//
//	GET /api/trace/meta                       ring geometry + field names
//	GET /api/trace/data?since=<idx>&max=<n>   columnar samples [since, writeIdx)
//	GET /api/trace/export?seconds=N&format=json|csv
//
// The daemon may run with --trace-seconds 0 (or be an older build): the
// segment is opened lazily and its absence is a 503, never a startup
// failure.
package traceapi

import (
	"encoding/json"
	"fmt"
	"log"
	"net/http"
	"os"
	"strconv"
	"sync"

	"codesys_dev/backend/internal/shm"
)

// field couples a column name to its extractor — the single place where
// sample fields are named; meta, JSON and CSV encoders all iterate this
// table, so the wire order is the table order.
type field struct {
	name string
	get  func(*shm.TraceSample) float64
}

var fields = buildFields()

func buildFields() []field {
	f := []field{
		{"cycle", func(s *shm.TraceSample) float64 { return float64(s.Cycle) }},
		{"tMonoNs", func(s *shm.TraceSample) float64 { return float64(s.TMonoNs) }},
		{"periodNs", func(s *shm.TraceSample) float64 { return float64(s.PeriodNs) }},
		{"exchangeNs", func(s *shm.TraceSample) float64 { return float64(s.ExchangeNs) }},
		{"busState", func(s *shm.TraceSample) float64 { return float64(s.BusState) }},
		{"statusBits", func(s *shm.TraceSample) float64 { return float64(s.StatusBits) }},
		{"runState", func(s *shm.TraceSample) float64 { return float64(s.RunState) }},
	}
	for i := range [4]struct{}{} {
		i := i // capture per axis
		p := fmt.Sprintf("axis%d.", i)
		f = append(f,
			field{p + "actPos", func(s *shm.TraceSample) float64 { return s.Axes[i].ActPos }},
			field{p + "actVel", func(s *shm.TraceSample) float64 { return s.Axes[i].ActVel }},
			field{p + "setPos", func(s *shm.TraceSample) float64 { return s.Axes[i].SetPos }},
			field{p + "setVel", func(s *shm.TraceSample) float64 { return s.Axes[i].SetVel }},
			field{p + "step", func(s *shm.TraceSample) float64 { return float64(s.Axes[i].Step) }},
			field{p + "flags", func(s *shm.TraceSample) float64 { return float64(s.Axes[i].Flags) }},
			field{p + "errorId", func(s *shm.TraceSample) float64 { return float64(s.Axes[i].ErrorID) }},
			field{p + "faultCode", func(s *shm.TraceSample) float64 { return float64(s.Axes[i].FaultCode) }},
			field{p + "driveStatus", func(s *shm.TraceSample) float64 { return float64(s.Axes[i].DriveStatus) }},
			field{p + "ioBits", func(s *shm.TraceSample) float64 { return float64(s.Axes[i].IOBits) }},
		)
	}
	return f
}

const (
	defaultBatch = 2048  // samples per /data response when max is omitted
	maxBatch     = 65536 // hard cap per response
)

// Handler lazily opens /dev/shm/plc_trace and re-opens it when the daemon
// recreates the segment (a restart unlinks + recreates it, which would
// otherwise leave us mmapped to the dead copy).
type Handler struct {
	// OpenPath is the segment path checked for staleness. Tests override it
	// together with Open.
	openPath string
	open     func() (*shm.TraceRing, error)

	mu   sync.Mutex
	ring *shm.TraceRing
	info os.FileInfo // identity of the mapped file, for staleness checks
}

// New returns a handler over the real /dev/shm segment.
func New() *Handler {
	return &Handler{openPath: "/dev/shm/" + shm.NamePlcTrace, open: shm.OpenTrace}
}

// NewWithOpener returns a handler over an arbitrary segment file — tests
// point it at a fixture written to a temp dir.
func NewWithOpener(path string, open func() (*shm.TraceRing, error)) *Handler {
	return &Handler{openPath: path, open: open}
}

// Register mounts the trace endpoints on mux.
func (h *Handler) Register(mux *http.ServeMux) {
	mux.HandleFunc("/api/trace/meta", h.meta)
	mux.HandleFunc("/api/trace/data", h.data)
	mux.HandleFunc("/api/trace/export", h.export)
}

// acquire returns a validated ring, (re)opening as needed.
func (h *Handler) acquire() (*shm.TraceRing, error) {
	h.mu.Lock()
	defer h.mu.Unlock()

	st, err := os.Stat(h.openPath)
	if err != nil {
		if h.ring != nil { // segment gone: daemon stopped with trace off
			h.ring.Close()
			h.ring, h.info = nil, nil
		}
		return nil, err
	}
	if h.ring != nil && h.info != nil && os.SameFile(h.info, st) {
		return h.ring, nil
	}
	if h.ring != nil { // recreated underneath us — remap
		h.ring.Close()
		h.ring, h.info = nil, nil
	}
	ring, err := h.open()
	if err != nil {
		return nil, err
	}
	h.ring, h.info = ring, st
	log.Printf("traceapi: mapped %s (capacity %d, period %d ns)",
		h.openPath, ring.Capacity(), ring.Header().PeriodNs)
	return ring, nil
}

func (h *Handler) withRing(w http.ResponseWriter, fn func(*shm.TraceRing)) {
	ring, err := h.acquire()
	if err != nil {
		http.Error(w, "trace segment unavailable (daemon without --trace-seconds?): "+err.Error(),
			http.StatusServiceUnavailable)
		return
	}
	fn(ring)
}

func (h *Handler) meta(w http.ResponseWriter, r *http.Request) {
	h.withRing(w, func(ring *shm.TraceRing) {
		hdr := ring.Header()
		names := make([]string, len(fields))
		for i, f := range fields {
			names[i] = f.name
		}
		writeJSON(w, map[string]any{
			"version":     hdr.Version,
			"periodNs":    hdr.PeriodNs,
			"sampleHz":    1e9 / float64(hdr.PeriodNs),
			"epochUnixNs": hdr.EpochUnixNs,
			"capacity":    hdr.Capacity,
			"writeIdx":    hdr.WriteIdx,
			"axes":        4,
			"fields":      names,
		})
	})
}

func (h *Handler) data(w http.ResponseWriter, r *http.Request) {
	h.withRing(w, func(ring *shm.TraceRing) {
		max := intParam(r, "max", defaultBatch)
		if max <= 0 || max > maxBatch {
			max = maxBatch
		}
		since := int64(-1)
		if s := r.URL.Query().Get("since"); s != "" {
			v, err := strconv.ParseInt(s, 10, 64)
			if err != nil {
				http.Error(w, "since must be an integer sample index (-1 = tail)", http.StatusBadRequest)
				return
			}
			since = v
		}
		var from uint64
		if since < 0 { // tail: start one batch back from the write head
			head := ring.Header().WriteIdx
			if head > uint64(max) {
				from = head - uint64(max)
			}
		} else {
			from = uint64(since)
		}

		samples, first, next, dropped := ring.ReadRange(from, max)
		writeJSON(w, map[string]any{
			"first":   first,
			"next":    next,
			"dropped": dropped,
			"columns": toColumns(samples),
		})
	})
}

func (h *Handler) export(w http.ResponseWriter, r *http.Request) {
	h.withRing(w, func(ring *shm.TraceRing) {
		hdr := ring.Header()
		seconds := intParam(r, "seconds", 60)
		n := int(uint64(seconds) * 1e9 / hdr.PeriodNs)
		if n < 1 {
			n = 1
		}
		if n > int(ring.Capacity()) {
			n = int(ring.Capacity())
		}
		var from uint64
		if hdr.WriteIdx > uint64(n) {
			from = hdr.WriteIdx - uint64(n)
		}
		samples, _, _, _ := ring.ReadRange(from, n)

		switch r.URL.Query().Get("format") {
		case "csv":
			w.Header().Set("Content-Type", "text/csv")
			w.Header().Set("Content-Disposition", `attachment; filename="plc_trace.csv"`)
			writeCSV(w, samples)
		default:
			w.Header().Set("Content-Type", "application/json")
			w.Header().Set("Content-Disposition", `attachment; filename="plc_trace.json"`)
			names := make([]string, len(fields))
			for i, f := range fields {
				names[i] = f.name
			}
			writeJSON(w, map[string]any{
				"format":      "plc-trace/1",
				"source":      map[string]any{"daemon": "motion-daemon", "cycleNs": hdr.PeriodNs, "axes": 4},
				"epochUnixNs": hdr.EpochUnixNs,
				"fields":      names,
				"columns":     toColumns(samples),
			})
		}
	})
}

// toColumns transposes samples into per-field arrays, in fields-table order —
// the shape uPlot and pandas both consume directly.
func toColumns(samples []shm.TraceSample) [][]float64 {
	cols := make([][]float64, len(fields))
	for i := range cols {
		cols[i] = make([]float64, len(samples))
	}
	for j := range samples {
		for i, f := range fields {
			cols[i][j] = f.get(&samples[j])
		}
	}
	return cols
}

func writeCSV(w http.ResponseWriter, samples []shm.TraceSample) {
	for i, f := range fields {
		if i > 0 {
			fmt.Fprint(w, ",")
		}
		fmt.Fprint(w, f.name)
	}
	fmt.Fprintln(w)
	for j := range samples {
		for i, f := range fields {
			if i > 0 {
				fmt.Fprint(w, ",")
			}
			fmt.Fprintf(w, "%g", f.get(&samples[j]))
		}
		fmt.Fprintln(w)
	}
}

func writeJSON(w http.ResponseWriter, v any) {
	w.Header().Set("Content-Type", "application/json")
	if err := json.NewEncoder(w).Encode(v); err != nil {
		log.Printf("traceapi: encode: %v", err)
	}
}

func intParam(r *http.Request, name string, def int) int {
	s := r.URL.Query().Get(name)
	if s == "" {
		return def
	}
	v, err := strconv.Atoi(s)
	if err != nil {
		return def
	}
	return v
}

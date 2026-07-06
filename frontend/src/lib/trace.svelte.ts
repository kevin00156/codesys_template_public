// Client for the bridge's /api/trace endpoints — the data source of the
// Watch and Trace panels (CODESYS watch/trace parity).
//
// The daemon samples every control cycle into a shm ring; the bridge serves
// it verbatim. This store polls /api/trace/data with a cursor every 150 ms,
// keeps a live window per field, optionally accumulates an unbounded
// recording, and imports/exports the self-describing `plc-trace/1` columnar
// JSON (same shape the server's /api/trace/export produces).
//
// The column buffers are deliberately NOT reactive ($state) — they hold up to
// window×sampleHz×47 numbers and mutate 6× a second. Charts subscribe to the
// `version` counter instead and read the buffers imperatively.

export interface TraceMeta {
  version: number
  periodNs: number
  sampleHz: number
  epochUnixNs: number
  capacity: number
  writeIdx: number
  axes: number
  fields: string[]
}

export interface TraceFile {
  format: string
  source: { daemon: string; cycleNs: number; axes: number }
  epochUnixNs: number
  fields: string[]
  columns: number[][]
}

const POLL_MS = 150
const MAX_BATCH = 8192

export const WINDOW_CHOICES = [10, 30, 60] as const

class TraceStore {
  meta = $state<TraceMeta | null>(null)
  /** Trace segment reachable (daemon runs with --trace-seconds > 0). */
  available = $state(false)
  recording = $state(false)
  windowSec = $state(30)
  /** Samples lost to ring overwrite since the panel opened. */
  dropped = $state(0)
  /** Newest sample as name→value — the Watch panel's data. */
  latest = $state<Record<string, number> | null>(null)
  /** Imported recording, viewed instead of the live stream when set. */
  file = $state<TraceFile | null>(null)
  fileError = $state('')
  /** Bumped after every poll that appended data; charts re-read on change. */
  version = $state(0)

  // Non-reactive buffers, parallel to meta.fields.
  live: number[][] = []
  rec: number[][] | null = null
  recStartUnixNs = 0

  #timer: ReturnType<typeof setInterval> | null = null
  #cursor = -1
  #polling = false

  start() {
    if (this.#timer) return
    this.#timer = setInterval(() => this.#poll(), POLL_MS)
    this.#poll()
  }

  stop() {
    if (this.#timer) { clearInterval(this.#timer); this.#timer = null }
  }

  async #poll() {
    if (this.#polling) return // don't stack requests on a slow link
    this.#polling = true
    try {
      if (!this.meta) {
        const r = await fetch('/api/trace/meta')
        if (!r.ok) { this.available = false; return }
        this.meta = await r.json()
        this.live = this.meta!.fields.map(() => [])
        this.#cursor = -1
      }
      const r = await fetch(`/api/trace/data?since=${this.#cursor}&max=${MAX_BATCH}`)
      if (!r.ok) {
        // Daemon restarted or trace turned off — drop the meta and re-probe.
        this.available = false
        this.meta = null
        return
      }
      const d: { next: number; dropped: number; columns: number[][] } = await r.json()
      this.available = true
      if (this.#cursor >= 0) this.dropped += d.dropped
      this.#cursor = d.next
      const n = d.columns[0]?.length ?? 0
      if (n === 0) return

      const cap = Math.ceil(this.windowSec * (this.meta!.sampleHz ?? 500))
      for (let i = 0; i < this.live.length; i++) {
        const col = this.live[i]
        col.push(...d.columns[i])
        if (col.length > cap) col.splice(0, col.length - cap)
        if (this.rec) this.rec[i].push(...d.columns[i])
      }

      const latest: Record<string, number> = {}
      this.meta!.fields.forEach((f, i) => { latest[f] = d.columns[i][n - 1] })
      this.latest = latest
      this.version++
    } catch {
      this.available = false
    } finally {
      this.#polling = false
    }
  }

  startRecording() {
    if (!this.meta) return
    this.rec = this.meta.fields.map(() => [])
    this.recStartUnixNs = this.meta.epochUnixNs
    this.recording = true
  }

  stopRecording() {
    this.recording = false
  }

  /** Recorded samples so far (0 when not recording). */
  get recLength(): number {
    return this.rec?.[0]?.length ?? 0
  }

  exportRecording() {
    if (!this.rec || !this.meta) return
    const file: TraceFile = {
      format: 'plc-trace/1',
      source: { daemon: 'motion-daemon', cycleNs: this.meta.periodNs, axes: this.meta.axes },
      epochUnixNs: this.recStartUnixNs,
      fields: this.meta.fields,
      columns: this.rec,
    }
    const blob = new Blob([JSON.stringify(file)], { type: 'application/json' })
    const a = document.createElement('a')
    a.href = URL.createObjectURL(blob)
    a.download = `plc_trace_${new Date().toISOString().replace(/[:.]/g, '-')}.json`
    a.click()
    URL.revokeObjectURL(a.href)
  }

  discardRecording() {
    this.rec = null
    this.recording = false
  }

  async importFile(f: globalThis.File) {
    this.fileError = ''
    try {
      const parsed = JSON.parse(await f.text()) as TraceFile
      if (parsed.format !== 'plc-trace/1' || !Array.isArray(parsed.fields) ||
          !Array.isArray(parsed.columns) || parsed.fields.length !== parsed.columns.length) {
        throw new Error('not a plc-trace/1 file')
      }
      this.file = parsed
    } catch (e) {
      this.fileError = `import failed: ${e instanceof Error ? e.message : e}`
    }
  }

  closeFile() {
    this.file = null
  }
}

export const trace = new TraceStore()

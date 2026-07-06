<script lang="ts">
  import { onMount, onDestroy } from 'svelte'
  import { trace, WINDOW_CHOICES } from './trace.svelte.ts'
  import TraceChart from './TraceChart.svelte'

  let selected = $state<string[]>(['axis0.actPos', 'axis0.setPos'])
  let fileInput: HTMLInputElement

  onMount(() => trace.start())
  onDestroy(() => trace.stop())

  const source = $derived(trace.file ? 'file' as const : 'live' as const)
  // cycle/tMonoNs are monotonic ramps — they are the x-axis, not signals.
  const plottable = (f: string) => f !== 'cycle' && f !== 'tMonoNs'
  const allFields = $derived((trace.file?.fields ?? trace.meta?.fields ?? []).filter(plottable))
  const groups = $derived.by(() => {
    const g: { name: string; fields: string[] }[] = [{ name: 'system', fields: [] }]
    for (const f of allFields) {
      const dot = f.indexOf('.')
      if (dot < 0) { g[0].fields.push(f); continue }
      const prefix = f.slice(0, dot)
      let grp = g.find(x => x.name === prefix)
      if (!grp) { grp = { name: prefix, fields: [] }; g.push(grp) }
      grp.fields.push(f)
    }
    return g
  })

  function toggle(f: string) {
    selected = selected.includes(f) ? selected.filter(x => x !== f) : [...selected, f]
  }

  async function onImport(e: Event) {
    const f = (e.target as HTMLInputElement).files?.[0]
    if (f) await trace.importFile(f)
    ;(e.target as HTMLInputElement).value = ''
  }
</script>

<details class="trace" open>
  <summary>Trace <span class="hint">{trace.available ? `${trace.meta?.sampleHz.toFixed(0) ?? '?'} Hz` : 'offline'}{trace.file ? ' — viewing file' : ''}</span></summary>

  {#if !trace.available && !trace.file}
    <div class="offline">
      Trace segment unavailable — run the daemon with <code>--trace-seconds N</code>.
      You can still <button class="link" onclick={() => fileInput.click()}>import a recording</button>.
    </div>
  {:else}
    <div class="toolbar">
      {#if trace.file}
        <span class="file-badge">file: {trace.file.columns[0]?.length ?? 0} samples @ {(1e9 / trace.file.source.cycleNs).toFixed(0)} Hz</span>
        <button class="btn" onclick={() => trace.closeFile()}>Back to live</button>
      {:else}
        <label>Window
          <select bind:value={trace.windowSec}>
            {#each WINDOW_CHOICES as w}<option value={w}>{w} s</option>{/each}
          </select>
        </label>

        {#if trace.recording}
          <button class="btn rec on" onclick={() => trace.stopRecording()}>■ Stop ({trace.recLength})</button>
        {:else}
          <button class="btn rec" onclick={() => trace.startRecording()}>● Record</button>
        {/if}
        {#if !trace.recording && trace.recLength > 0}
          <button class="btn" onclick={() => trace.exportRecording()}>Export recording</button>
          <button class="btn" onclick={() => trace.discardRecording()}>Discard</button>
        {/if}

        <span class="spacer"></span>
        <a class="btn" href="/api/trace/export?seconds=60" download>Server dump 60s</a>
        <a class="btn" href="/api/trace/export?seconds=60&format=csv" download>CSV</a>
      {/if}
      <button class="btn" onclick={() => fileInput.click()}>Import…</button>
      {#if trace.dropped > 0 && !trace.file}
        <span class="dropped" title="samples lost to ring overwrite">dropped: {trace.dropped}</span>
      {/if}
    </div>

    <div class="body">
      <div class="picker">
        {#each groups as g}
          <div class="group">
            <div class="group-name">{g.name}</div>
            {#each g.fields as f}
              <label class="field">
                <input type="checkbox" checked={selected.includes(f)} onchange={() => toggle(f)} />
                <span>{f.includes('.') ? f.slice(f.indexOf('.') + 1) : f}</span>
              </label>
            {/each}
          </div>
        {/each}
      </div>
      <div class="chart-area">
        <TraceChart {selected} {source} />
      </div>
    </div>
  {/if}

  {#if trace.fileError}<div class="error">{trace.fileError}</div>{/if}
  <input type="file" accept=".json,application/json" bind:this={fileInput} onchange={onImport} hidden />
</details>

<style>
  .trace { background: #1e293b; border-radius: 8px; padding: 0.5rem 0.75rem; }
  summary { cursor: pointer; font-size: 0.8rem; font-weight: 600; color: #94a3b8; text-transform: uppercase; letter-spacing: 0.05em; }
  .hint { font-weight: 400; text-transform: none; color: #64748b; margin-left: 0.5rem; }

  .offline { color: #94a3b8; font-size: 0.85rem; padding: 0.75rem 0; }
  .offline code { background: #0f172a; padding: 0 4px; border-radius: 3px; }
  .link { background: none; border: none; color: #60a5fa; cursor: pointer; padding: 0; font-size: inherit; text-decoration: underline; }

  .toolbar { display: flex; align-items: center; gap: 0.5rem; flex-wrap: wrap; margin: 0.6rem 0; font-size: 0.8rem; color: #94a3b8; }
  .toolbar label { display: inline-flex; align-items: center; gap: 0.35rem; }
  .toolbar select { background: #0f172a; color: #e2e8f0; border: 1px solid #334155; border-radius: 4px; padding: 0.15rem 0.3rem; }
  .spacer { flex: 1; }
  .btn { background: #374151; color: #d1d5db; border: none; border-radius: 4px; padding: 0.25rem 0.6rem; font-size: 0.78rem; cursor: pointer; text-decoration: none; }
  .btn:hover { background: #4b5563; }
  .btn.rec { color: #f87171; }
  .btn.rec.on { background: #7f1d1d; color: #fecaca; }
  .file-badge { background: #1d4ed8; color: #dbeafe; border-radius: 4px; padding: 0.15rem 0.5rem; font-size: 0.75rem; }
  .dropped { color: #fbbf24; font-size: 0.75rem; }
  .error { color: #f87171; font-size: 0.8rem; margin-top: 0.4rem; }

  .body { display: flex; gap: 0.75rem; align-items: stretch; }
  .picker {
    flex: 0 0 170px; max-height: 300px; overflow-y: auto;
    background: #0f172a; border-radius: 8px; padding: 0.5rem;
  }
  .group-name { font-size: 0.68rem; color: #64748b; text-transform: uppercase; letter-spacing: 0.05em; margin: 0.4rem 0 0.15rem; }
  .group:first-child .group-name { margin-top: 0; }
  .field { display: flex; align-items: center; gap: 0.35rem; font-size: 0.78rem; color: #cbd5e1; padding: 0.08rem 0; cursor: pointer; }
  .field input { accent-color: #1d4ed8; }
  .chart-area { flex: 1; min-width: 0; }
</style>

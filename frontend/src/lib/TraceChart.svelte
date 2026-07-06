<script lang="ts">
  import { onDestroy } from 'svelte'
  import uPlot from 'uplot'
  import 'uplot/dist/uPlot.min.css'
  import { trace } from './trace.svelte.ts'

  let { selected, source }: { selected: string[]; source: 'live' | 'file' } = $props()

  let container: HTMLDivElement
  let chart: uPlot | null = null

  const TICK = '#94a3b8'
  const GRID = '#1e293b'
  const axisBase = { stroke: TICK, ticks: { stroke: TICK, width: 1 }, grid: { stroke: GRID, width: 1 } }
  const PALETTE = ['#60a5fa', '#f87171', '#4ade80', '#fbbf24', '#c084fc', '#22d3ee', '#fb923c', '#f472b6', '#a3e635', '#94a3b8']

  // Unit class decides which y-scale a field plots on, so positions don't
  // squash velocities and nanosecond timings live on their own axis.
  function scaleOf(name: string): string {
    if (name.endsWith('Pos')) return 'pos'
    if (name.endsWith('Vel')) return 'vel'
    if (name.endsWith('Ns')) return 'ns'
    return 'misc'
  }

  function sourceData(): { fields: string[]; columns: number[][]; epochNs: number } | null {
    if (source === 'file') {
      const f = trace.file
      return f ? { fields: f.fields, columns: f.columns, epochNs: f.epochUnixNs } : null
    }
    const m = trace.meta
    return m ? { fields: m.fields, columns: trace.live, epochNs: m.epochUnixNs } : null
  }

  function buildData(): uPlot.AlignedData | null {
    const d = sourceData()
    if (!d) return null
    const ti = d.fields.indexOf('tMonoNs')
    if (ti < 0 || !d.columns[ti]) return null
    const xs = d.columns[ti].map(t => (d.epochNs + t) / 1e9)
    const ys = selected.map(name => {
      const i = d.fields.indexOf(name)
      return i >= 0 ? d.columns[i] : xs.map(() => null as unknown as number)
    })
    return [xs, ...ys]
  }

  function buildChart() {
    chart?.destroy()
    chart = null
    if (!container || selected.length === 0) return

    const classes = [...new Set(selected.map(scaleOf))]
    const axes: uPlot.Axis[] = [{ ...axisBase }]
    if (classes.includes('pos')) axes.push({ ...axisBase, scale: 'pos', label: 'pos', size: 60 })
    if (classes.includes('vel')) axes.push({ ...axisBase, scale: 'vel', label: 'vel', side: 1, size: 60 })
    if (classes.includes('ns')) axes.push({ ...axisBase, scale: 'ns', label: 'ns', side: 1, size: 70 })
    if (classes.includes('misc')) axes.push({ ...axisBase, scale: 'misc', side: 1, size: 50 })

    const opts: uPlot.Options = {
      width: container.clientWidth - 16 || 800,
      height: 260,
      scales: { x: { time: true } },
      series: [
        {},
        ...selected.map((name, i) => ({
          label: name,
          stroke: PALETTE[i % PALETTE.length],
          width: 1.5,
          scale: scaleOf(name),
          points: { show: false },
        })),
      ],
      axes,
    }
    chart = new uPlot(opts, buildData() ?? [[]], container)
  }

  // Recreate when the series set (or data source) changes…
  $effect(() => {
    void selected.join('\0')
    void source
    void trace.file
    buildChart()
  })

  // …and refresh data in place on every live poll tick.
  $effect(() => {
    void trace.version
    if (chart && source === 'live') {
      const d = buildData()
      if (d) chart.setData(d)
    }
  })

  onDestroy(() => chart?.destroy())
</script>

<div bind:this={container} class="chart-wrap">
  {#if selected.length === 0}
    <div class="empty">Select variables to plot</div>
  {/if}
</div>

<style>
  .chart-wrap {
    width: 100%; overflow: hidden; min-height: 60px;
    background: #0f172a; border-radius: 8px; padding: 0.5rem 0.5rem 0;
  }
  .chart-wrap :global(.uplot)    { width: 100% !important; }
  .chart-wrap :global(.u-legend) { color: #94a3b8; font-size: 0.75rem; }
  .chart-wrap :global(.u-label)  { color: #94a3b8; }
  .chart-wrap :global(.u-value)  { color: #e2e8f0; }
  .empty { color: #475569; text-align: center; padding: 1.5rem; font-size: 0.85rem; }
</style>

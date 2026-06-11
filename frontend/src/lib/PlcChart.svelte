<script lang="ts">
  import { onMount, onDestroy } from 'svelte'
  import uPlot from 'uplot'
  import 'uplot/dist/uPlot.min.css'

  interface HistoryPoint { ts: number; temperature: number }
  let { history }: { history: HistoryPoint[] } = $props()

  let container: HTMLDivElement
  let chart: uPlot | null = null

  const TICK = '#94a3b8'
  const GRID = '#1e293b'
  const axisBase = { stroke: TICK, ticks: { stroke: TICK, width: 1 }, grid: { stroke: GRID, width: 1 } }

  const opts: uPlot.Options = {
    width: 800,
    height: 180,
    title: 'System Temperature (30s)',
    scales: { x: { time: true } },
    series: [
      {},
      { label: 'Temperature', stroke: '#ef4444', width: 2, scale: 'temp' },
    ],
    axes: [
      { ...axisBase },
      { ...axisBase, scale: 'temp', label: '°C', side: 1, size: 60 },
    ],
  }

  function toUplotData(pts: HistoryPoint[]): uPlot.AlignedData {
    return [pts.map(p => p.ts / 1000), pts.map(p => p.temperature)]
  }

  onMount(() => { chart = new uPlot(opts, toUplotData(history), container) })
  onDestroy(() => chart?.destroy())

  $effect(() => {
    if (chart && history.length > 0) chart.setData(toUplotData(history))
  })
</script>

<div bind:this={container} class="chart-wrap"></div>

<style>
  .chart-wrap {
    width: 100%; overflow: hidden;
    background: #1e293b; border-radius: 8px; padding: 0.5rem 0.5rem 0;
  }
  .chart-wrap :global(.uplot)    { width: 100% !important; }
  .chart-wrap :global(.u-title)  { color: #94a3b8; font-size: 0.85rem; font-weight: 600; letter-spacing: 0.05em; text-transform: uppercase; }
  .chart-wrap :global(.u-legend) { color: #94a3b8; font-size: 0.8rem; }
  .chart-wrap :global(.u-label)  { color: #94a3b8; }
  .chart-wrap :global(.u-value)  { color: #e2e8f0; }
</style>

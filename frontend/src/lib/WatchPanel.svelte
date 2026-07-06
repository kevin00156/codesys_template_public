<script lang="ts">
  // Live watch of daemon internals — the values that are NOT in PlcData/WS
  // (those already fill the dashboard above): cycle timing, bus state,
  // drive status/fault codes and raw IO bits, straight from the newest trace
  // sample. The CODESYS counterpart is the online variable display.
  import { trace } from './trace.svelte.ts'

  const BUS = ['Init', 'PreOp', 'SafeOp', 'Op']
  const DRIVE = ['Offline', 'Disabled', 'Enabling', 'Enabled', 'QuickStop', 'Fault']
  const IO = ['PosLim', 'NegLim', 'Homed', 'Enable', 'FaultReset']

  const v = $derived(trace.latest)
  const axes = $derived.by(() => {
    if (!v) return []
    const n = trace.meta?.axes ?? 4
    return Array.from({ length: n }, (_, i) => ({
      i,
      fault: v[`axis${i}.faultCode`] ?? 0,
      drive: DRIVE[v[`axis${i}.driveStatus`] ?? 0] ?? '?',
      io: v[`axis${i}.ioBits`] ?? 0,
    }))
  })

  const us = (ns: number | undefined) => ns === undefined ? '—' : (ns / 1000).toFixed(0)
</script>

<details class="watch" open>
  <summary>Watch — daemon internals <span class="hint">{trace.available ? 'live' : 'offline'}</span></summary>
  {#if v}
    <div class="row">
      <div class="cell"><span class="k">cycle</span><span class="val">{v['cycle']}</span></div>
      <div class="cell"><span class="k">period</span><span class="val">{us(v['periodNs'])} <em>µs</em></span></div>
      <div class="cell"><span class="k">exchange</span><span class="val">{us(v['exchangeNs'])} <em>µs</em></span></div>
      <div class="cell"><span class="k">bus</span><span class="val">{BUS[v['busState'] ?? 0] ?? '?'}</span></div>
      <div class="cell" class:bad={((v['statusBits'] ?? 0) & 1) !== 0}>
        <span class="k">exchange err</span><span class="val">{((v['statusBits'] ?? 0) & 1) ? 'YES' : 'no'}</span>
      </div>
      <div class="cell"><span class="k">cmd valid</span><span class="val">{((v['statusBits'] ?? 0) & 4) ? 'yes' : 'no'}</span></div>
    </div>
    <table>
      <thead><tr><th>Axis</th><th>Drive</th><th>Fault</th><th>IO</th></tr></thead>
      <tbody>
        {#each axes as ax}
          <tr>
            <td class="ax-id">{ax.i}</td>
            <td class:bad={ax.drive === 'Fault'}>{ax.drive}</td>
            <td class="mono" class:bad={ax.fault !== 0}>{ax.fault === 0 ? '—' : '0x' + ax.fault.toString(16).toUpperCase()}</td>
            <td>
              {#each IO as name, b}
                <span class="chip" class:on={(ax.io & (1 << b)) !== 0}>{name}</span>
              {/each}
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  {:else}
    <div class="offline">No trace data — run the daemon with <code>--trace-seconds N</code>.</div>
  {/if}
</details>

<style>
  .watch { background: #1e293b; border-radius: 8px; padding: 0.5rem 0.75rem; }
  summary { cursor: pointer; font-size: 0.8rem; font-weight: 600; color: #94a3b8; text-transform: uppercase; letter-spacing: 0.05em; }
  .hint { font-weight: 400; text-transform: none; color: #64748b; margin-left: 0.5rem; }
  .offline { color: #94a3b8; font-size: 0.85rem; padding: 0.5rem 0; }
  .offline code { background: #0f172a; padding: 0 4px; border-radius: 3px; }

  .row { display: flex; flex-wrap: wrap; gap: 0.5rem; margin: 0.6rem 0; }
  .cell { background: #0f172a; border-radius: 6px; padding: 0.35rem 0.6rem; display: flex; flex-direction: column; min-width: 90px; }
  .cell.bad { border: 1px solid #b91c1c; }
  .k { font-size: 0.65rem; color: #64748b; text-transform: uppercase; letter-spacing: 0.05em; }
  .val { font-size: 0.95rem; font-weight: 600; font-variant-numeric: tabular-nums; }
  .val em { font-style: normal; font-size: 0.7rem; color: #64748b; }

  table { width: 100%; border-collapse: collapse; font-size: 0.82rem; }
  th { padding: 0.3rem 0.5rem; color: #64748b; font-weight: 600; text-align: left; font-size: 0.7rem; text-transform: uppercase; border-bottom: 1px solid #334155; }
  td { padding: 0.3rem 0.5rem; border-bottom: 1px solid #263449; }
  tbody tr:last-child td { border-bottom: none; }
  .ax-id { font-weight: 700; color: #94a3b8; }
  .mono { font-family: monospace; }
  .bad { color: #f87171; }
  .chip { display: inline-block; background: #111827; color: #4b5563; border-radius: 3px; padding: 0 5px; font-size: 0.68rem; margin-right: 3px; }
  .chip.on { background: #14532d; color: #4ade80; }
</style>

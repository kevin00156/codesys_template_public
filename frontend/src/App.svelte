<script lang="ts">
  import { onMount } from 'svelte'
  import { ws } from './lib/ws.svelte.ts'
  import { i18n, LOCALES, t } from './lib/i18n/store.svelte.ts'
  import { auth } from './lib/auth.svelte.ts'
  import Login from './lib/Login.svelte'
  import PlcChart from './lib/PlcChart.svelte'
  import ControlPanel from './lib/ControlPanel.svelte'

  onMount(() => {
    auth.checkStatus() // is auth on, and are we already logged in?
    ws.connect()
  })

  // enumAxisControl_Step labels (matches IEC enum)
  const stepLabel = (s: number): string => ({
    0: 'Idle', 10: 'Enabling', 20: 'Ready', 25: 'Not Ready',
    30: 'Jog+', 31: 'Jog-', 40: 'MoveAbs', 50: 'MoveRel',
    60: 'MoveVel', 70: 'Homing…', 71: 'Homing', 80: 'SetPos',
    90: 'Stopping', 91: 'Wait Stop', 95: 'Reset'
  }[s] ?? `Step ${s}`)

  const flagBit = (flags: number, bit: number) => !!(flags & (1 << bit))

  // $derived so the logout label re-translates when the locale switches.
  const roleLabel = $derived(t('common.role.' + (auth.role || 'operator')))
</script>

<header>
  <div class="brand">
    <span class="title">{t('demo.title')}</span>
    <span class="tagline">{t('demo.tagline')}</span>
  </div>
  <div class="header-right">
    <span class="conn" class:online={ws.connected}>{ws.connected ? 'Connected' : 'Disconnected'}</span>
    <div class="lang">
      {#each LOCALES as l}
        <button class="lang-btn" class:on={i18n.locale === l.id} onclick={() => i18n.setLocale(l.id)}>{l.label}</button>
      {/each}
    </div>
    {#if auth.enabled && auth.loggedIn}
      <button class="btn" onclick={() => auth.logout()}>{t('nav.logout', { role: roleLabel })}</button>
    {/if}
  </div>
</header>

{#if auth.usingDefault}
  <div class="pw-warning">{t('login.defaultBanner', { pw: auth.defaultPassword })}</div>
{/if}

{#if !auth.checked}
  <div class="waiting">…</div>
{:else}
<main>
  {#if ws.data}
    {@const sys = ws.data.system}
    {@const mac = ws.data.machine}
    {@const pro = ws.data.production}

    <!-- System -->
    <section class="group-label">System</section>
    <section class="card-row">
      <div class="card">
        <div class="label">Temperature</div>
        <div class="value">{sys.temperature.toFixed(1)} <span class="unit">°C</span></div>
      </div>
      <div class="card">
        <div class="label">Status</div>
        <div class="value mono">0x{sys.statusFlags.toString(16).toUpperCase().padStart(8,'0')}</div>
      </div>
      <div class="card" class:alarm={sys.alarmFlags !== 0}>
        <div class="label">Alarms</div>
        <div class="value mono">{sys.alarmFlags === 0 ? 'None' : `0x${sys.alarmFlags.toString(16).toUpperCase()}`}</div>
      </div>
      <div class="card">
        <div class="label">Run State</div>
        <div class="value">{mac.runState}</div>
      </div>
      <div class="card sp">
        <div class="label">Production State <span class="badge">PLC</span></div>
        <div class="value">{pro.nProductionState}</div>
      </div>
    </section>

    <!-- Machine axes -->
    <section class="group-label">Machine — Axes</section>
    <div class="axis-table-wrap">
      <table class="axis-table">
        <thead>
          <tr>
            <th>Axis</th>
            <th>State</th>
            <th>Act Pos</th>
            <th>Act Vel</th>
            <th>Set Pos</th>
            <th>Set Vel</th>
            <th>Flags</th>
            <th>Error</th>
          </tr>
        </thead>
        <tbody>
          {#each mac.axes as ax, i}
            <tr class:ax-error={flagBit(ax.flags, 2)}>
              <td class="ax-id">{i}</td>
              <td class="ax-step">{stepLabel(ax.step)}</td>
              <td class="num">{ax.actPos.toFixed(3)}</td>
              <td class="num">{ax.actVel.toFixed(3)}</td>
              <td class="num">{ax.setPos.toFixed(3)}</td>
              <td class="num">{ax.setVel.toFixed(3)}</td>
              <td class="mono">{ax.flags.toString(2).padStart(6,'0')}</td>
              <td class="num" class:err={ax.errorId !== 0}>{ax.errorId}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>

    <!-- Chart (system temperature history) -->
    <PlcChart history={ws.history} />

  {:else}
    <div class="waiting">{ws.connected ? 'Waiting for PLC data…' : 'Connecting…'}</div>
  {/if}

  <!-- Control — the read-only dashboard above is open to everyone; operating
       the machine requires a login. When auth is disabled loggedIn is true. -->
  {#if auth.loggedIn}
    <ControlPanel />
  {:else}
    <Login />
  {/if}
</main>
{/if}

<style>
  header {
    display: flex; justify-content: space-between; align-items: center;
    padding: 0.75rem 1.5rem; background: #1e293b; border-bottom: 1px solid #334155;
  }
  .brand { display: flex; flex-direction: column; gap: 0.1rem; }
  .title { font-size: 1.1rem; font-weight: 600; }
  .tagline { font-size: 0.72rem; color: #64748b; }
  .header-right { display: flex; align-items: center; gap: 0.75rem; }
  .conn { font-size: 0.8rem; padding: 0.2rem 0.6rem; border-radius: 999px; background: #374151; color: #9ca3af; }
  .conn.online { background: #14532d; color: #4ade80; }
  .lang { display: inline-flex; gap: 0.25rem; }
  .lang-btn { background: #374151; color: #9ca3af; border: none; border-radius: 4px; padding: 0.2rem 0.55rem; font-size: 0.75rem; cursor: pointer; }
  .lang-btn.on { background: #1d4ed8; color: #fff; }

  .pw-warning {
    padding: 0.5rem 1.5rem; font-size: 0.82rem;
    background: #422006; color: #fbbf24; border-bottom: 1px solid #854d0e;
  }

  main { padding: 1rem 1.5rem; display: flex; flex-direction: column; gap: 0.75rem; }

  .group-label { font-size: 0.7rem; color: #64748b; text-transform: uppercase; letter-spacing: 0.08em; margin-bottom: -0.25rem; }

  .card-row { display: flex; flex-wrap: wrap; gap: 0.75rem; }
  .card { background: #1e293b; border-radius: 8px; padding: 0.75rem 1rem; min-width: 140px; }
  .card.alarm { border: 1px solid #f97316; }
  .card.sp { border: 1px solid #334155; }
  .label { font-size: 0.72rem; color: #64748b; text-transform: uppercase; letter-spacing: 0.05em; margin-bottom: 0.25rem; }
  .value { font-size: 1.3rem; font-weight: 700; font-variant-numeric: tabular-nums; }
  .value.mono { font-family: monospace; font-size: 1rem; }
  .unit { font-size: 0.85rem; color: #64748b; font-weight: 400; }
  .badge { font-size: 0.6rem; background: #334155; color: #94a3b8; border-radius: 3px; padding: 0 4px; vertical-align: middle; margin-left: 4px; }

  .axis-table-wrap { overflow-x: auto; background: #1e293b; border-radius: 8px; }
  .axis-table { width: 100%; border-collapse: collapse; font-size: 0.85rem; }
  .axis-table th { padding: 0.5rem 0.75rem; color: #64748b; font-weight: 600; text-align: left; border-bottom: 1px solid #334155; font-size: 0.72rem; text-transform: uppercase; }
  .axis-table td { padding: 0.4rem 0.75rem; border-bottom: 1px solid #1e293b; }
  .axis-table tbody tr:last-child td { border-bottom: none; }
  .axis-table tr.ax-error { background: #1c1017; }
  .ax-id { font-weight: 700; color: #94a3b8; }
  .ax-step { color: #e2e8f0; }
  .num { font-variant-numeric: tabular-nums; text-align: right; }
  .mono { font-family: monospace; }
  .err { color: #f87171; }

  .waiting { color: #64748b; padding: 3rem; text-align: center; font-size: 1.1rem; }
</style>

<script lang="ts">
  import { ws } from './ws.svelte.ts'

  let selAxis    = $state(0)
  let jogVel     = $state(10.0)
  let moveAbsPos = $state(0.0)
  let moveAbsVel = $state(10.0)
  let prodState  = $state(0)

  // Sync production state input when PLC value changes (first connect only)
  let prodSynced = $state(false)
  $effect(() => {
    if (ws.data && !prodSynced) {
      prodState = ws.data.production.nProductionState
      prodSynced = true
    }
  })

  // Jog dead-man, browser side. The backend clears jog bits not refreshed
  // within its -jog-timeout window, so while a jog button is held we re-send
  // the command every 250ms; every release path (pointerup, pointercancel,
  // capture loss, WS drop, unmount) sends stop and ends the refresh.
  const JOG_RESEND_MS = 250
  let jogTimer: ReturnType<typeof setInterval> | null = null
  let jogAxis = 0

  function startJog(e: PointerEvent, flags: number) {
    // Capture the pointer so release outside the button still fires pointerup.
    ;(e.currentTarget as HTMLElement).setPointerCapture(e.pointerId)
    stopJog()
    jogAxis = selAxis
    const axis = jogAxis
    const send = () => ws.sendAxisCmd(axis, flags, jogVel)
    send()
    jogTimer = setInterval(send, JOG_RESEND_MS)
  }

  function stopJog() {
    if (jogTimer === null) return
    clearInterval(jogTimer)
    jogTimer = null
    ws.sendAxisCmd(jogAxis, 0)
  }

  // WS drop: stop resending immediately; the backend watchdog clears the
  // level-held jog bits since our keepalives no longer arrive.
  $effect(() => {
    if (!ws.connected && jogTimer !== null) {
      clearInterval(jogTimer)
      jogTimer = null
    }
  })

  $effect(() => () => stopJog())
</script>

<section class="panel">
  <h2>Control</h2>

  <!-- Machine-level -->
  <div class="group">
    <span class="sub">Machine</span>
    <button class="btn-sm" onclick={() => ws.sendMachineCtrl(1)}>Reset</button>
    <button class="btn-sm danger" onclick={() => ws.sendMachineCtrl(2)}>EMS</button>
    <button class="btn-sm" onclick={() => ws.sendMachineCtrl(4)}>System Run</button>
  </div>

  <!-- Per-axis -->
  <div class="group">
    <span class="sub">Axis</span>
    <label>
      #
      <select bind:value={selAxis}>
        {#each [0,1,2,3] as i}<option value={i}>{i}</option>{/each}
      </select>
    </label>
    <button class="btn-sm" onclick={() => ws.sendAxisCmd(selAxis, 1)}>Enable</button>
    <button class="btn-sm" onclick={() => ws.sendAxisCmd(selAxis, 4)}>Reset</button>
    <button class="btn-sm" onclick={() => ws.sendAxisCmd(selAxis, 8)}>Stop</button>
    <button class="btn-sm" onclick={() => ws.sendAxisCmd(selAxis, 2)}>Home</button>
  </div>

  <div class="group">
    <span class="sub">Jog</span>
    <label>Vel <input type="number" step="1" bind:value={jogVel} class="num-in" /></label>
    <button class="btn-sm" onpointerdown={(e) => startJog(e, 16)}
                           onpointerup={stopJog} onpointercancel={stopJog}
                           onlostpointercapture={stopJog}>Jog +</button>
    <button class="btn-sm" onpointerdown={(e) => startJog(e, 32)}
                           onpointerup={stopJog} onpointercancel={stopJog}
                           onlostpointercapture={stopJog}>Jog −</button>
  </div>

  <div class="group">
    <span class="sub">Move Abs</span>
    <label>Pos <input type="number" step="0.1" bind:value={moveAbsPos} class="num-in" /></label>
    <label>Vel <input type="number" step="1"   bind:value={moveAbsVel} class="num-in" /></label>
    <button class="btn-sm" onclick={() => ws.sendAxisCmd(selAxis, 64, 0, moveAbsPos, moveAbsVel)}>Go</button>
  </div>

  <!-- Production state -->
  <div class="group">
    <span class="sub">Production</span>
    <label>State <input type="number" bind:value={prodState} class="num-in" /></label>
    <button class="btn-sm" onclick={() => ws.sendProduction(prodState)}>Apply</button>
  </div>

  {#if ws.lastAck}
    <p class="ack" class:ok={ws.lastAck.ok} class:err={!ws.lastAck.ok}>
      {ws.lastAck.ok ? '✓ Accepted' : `✗ ${ws.lastAck.error}`}
    </p>
  {/if}
</section>

<style>
  .panel { background: #1e293b; border-radius: 8px; padding: 1rem 1.5rem; }
  h2 { font-size: 0.75rem; color: #64748b; text-transform: uppercase; letter-spacing: 0.08em; margin-bottom: 0.75rem; }
  .group { display: flex; align-items: center; gap: 0.5rem; flex-wrap: wrap; margin-bottom: 0.5rem; }
  .sub { font-size: 0.75rem; color: #64748b; min-width: 5rem; }
  label { display: flex; align-items: center; gap: 0.3rem; font-size: 0.8rem; color: #94a3b8; }
  select, .num-in {
    background: #0f172a; border: 1px solid #334155; color: #e2e8f0;
    border-radius: 4px; padding: 0.25rem 0.4rem; font-size: 0.9rem;
  }
  .num-in { width: 6rem; }
  select { width: 3.5rem; }
  .btn-sm {
    background: #334155; color: #e2e8f0; border: none; border-radius: 4px;
    padding: 0.3rem 0.7rem; cursor: pointer; font-size: 0.82rem;
  }
  .btn-sm:hover { background: #3b82f6; }
  .btn-sm.danger { background: #7f1d1d; }
  .btn-sm.danger:hover { background: #dc2626; }
  .ack { font-size: 0.82rem; margin-top: 0.25rem; }
  .ok { color: #4ade80; }
  .err { color: #f87171; }
</style>

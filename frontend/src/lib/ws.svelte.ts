export interface SystemState {
  temperature: number
  statusFlags: number
  alarmFlags: number
}

export interface AxisState {
  actPos:  number
  actVel:  number
  setPos:  number
  setVel:  number
  step:    number
  flags:   number
  errorId: number
}

export interface MachineState {
  axes:     [AxisState, AxisState, AxisState, AxisState]
  runState: number
  alarms:   number
}

export interface ProductionState {
  nProductionState: number
}

export interface PlcData {
  type:       'data'
  ts:         number
  system:     SystemState
  machine:    MachineState
  production: ProductionState
}

import { auth } from './auth.svelte.ts'

const HISTORY_LEN = 300

class WsStore {
  data      = $state<PlcData | null>(null)
  connected = $state(false)
  lastAck   = $state<{ ok: boolean; error?: string } | null>(null)
  history   = $state<{ ts: number; temperature: number }[]>([])

  #ws: WebSocket | null = null
  #retryTimer: ReturnType<typeof setTimeout> | null = null

  connect() {
    const url = `${location.protocol === 'https:' ? 'wss:' : 'ws:'}//${location.host}/ws`
    const ws = new WebSocket(url)
    this.#ws = ws

    ws.onopen = () => {
      this.connected = true
      if (this.#retryTimer) { clearTimeout(this.#retryTimer); this.#retryTimer = null }
    }
    ws.onclose = () => {
      this.connected = false
      this.#retryTimer = setTimeout(() => this.connect(), 2000)
    }
    ws.onerror = () => ws.close()
    ws.onmessage = (e) => {
      const msg = JSON.parse(e.data)
      if (msg.type === 'data') {
        this.data = msg as PlcData
        const h = this.history
        h.push({ ts: msg.ts, temperature: msg.system.temperature })
        if (h.length > HISTORY_LEN) h.splice(0, h.length - HISTORY_LEN)
      } else if (msg.type === 'ack') {
        this.lastAck = { ok: msg.ok, error: msg.error }
        // Backend rejected a command because the session is gone/expired — drop
        // back to the login screen (mirrors the HTTP-401 path in auth store).
        if (!msg.ok && msg.error === 'unauthorized') auth.onUnauthorized()
      }
    }
  }

  send(cmd: object) {
    if (this.#ws?.readyState === WebSocket.OPEN) this.#ws.send(JSON.stringify(cmd))
  }

  sendMachineCtrl(controlFlags: number) {
    this.send({ type: 'machine', controlFlags })
  }

  sendAxisCmd(axisIndex: number, axisFlags: number, jogVel = 0, moveAbsPos = 0, moveAbsVel = 0) {
    this.send({ type: 'axis', axisIndex, axisFlags, jogVel, moveAbsPos, moveAbsVel })
  }

  sendProduction(nProductionState: number) {
    this.send({ type: 'production', nProductionState })
  }
}

export const ws = new WsStore()

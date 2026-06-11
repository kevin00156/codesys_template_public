// Auth store — mirrors the backend's role-password gates.
//
// This is presentation only: the real wall is the backend's Wrap middleware
// (see backend/internal/auth). Here we just decide whether to show the login
// screen instead of the app, and react when a protected request comes back 401
// (cookie expired, or the backend restarted and dropped sessions).
//
// The session itself lives in an HttpOnly cookie the browser sends automatically;
// JS never sees the token, so there is nothing to persist here.

import { t } from './i18n/store.svelte.ts'

class AuthStore {
  enabled  = $state(false)   // backend has a password configured
  loggedIn = $state(false)   // we hold a valid session
  checked  = $state(false)   // status fetched at least once
  error    = $state('')
  // 操作員級是否啟用（後端設了 PLC_BRIDGE_OPERATOR_HASH 才會把機器/訂單
  // 寫入納入門禁）。false 時主控面維持舊行為：免登入。
  operatorGated = $state(false)
  // 角色（意見稿 §8.3）：'' 未登入；'operator' 操作員（主控/訂單）；
  // 'tuner' 調機（+調試）；'vendor' 廠商（全開）。auth 未啟用時一律視為 vendor。
  role     = $state<'' | 'operator' | 'tuner' | 'vendor'>('')

  /** 某分頁需要的角色是否被目前 session 滿足。 */
  satisfies(required: 'operator' | 'tuner' | 'vendor'): boolean {
    if (!this.enabled) return true
    if (required === 'operator') return !this.operatorGated || this.role !== ''
    if (required === 'tuner') return this.role === 'tuner' || this.role === 'vendor'
    return this.role === 'vendor'
  }

  /** One-shot probe at startup: is auth on, and are we already in? */
  async checkStatus() {
    try {
      const res = await fetch('/api/auth/status')
      const j = await res.json()
      this.enabled       = !!j.enabled
      this.loggedIn      = !!j.loggedIn
      this.operatorGated = !!j.operatorGated
      this.role          = parseRole(j.role)
    } catch {
      // Backend unreachable — leave defaults (enabled=false). Nothing works in
      // that state anyway; the backend is the gate, not this flag.
    } finally {
      this.checked = true
    }
  }

  async login(password: string): Promise<boolean> {
    this.error = ''
    try {
      const res = await fetch('/api/login', {
        method:  'POST',
        headers: { 'Content-Type': 'application/json' },
        body:    JSON.stringify({ password }),
      })
      const j = await res.json().catch(() => null)
      if (!res.ok) {
        this.error = j?.error ?? t('login.failed', { status: res.status })
        return false
      }
      this.loggedIn = true
      this.role = parseRole(j?.role)
      return true
    } catch (e: any) {
      this.error = e?.message ?? t('login.networkError')
      return false
    }
  }

  async logout() {
    try { await fetch('/api/logout', { method: 'POST' }) } catch { /* best effort */ }
    this.loggedIn = false
    this.role = ''
  }

  /** Called by API clients when a protected request returns 401, so the UI
   *  drops back to the login screen instead of silently failing. */
  onUnauthorized() {
    if (this.loggedIn) { this.loggedIn = false; this.role = '' }
  }
}

function parseRole(v: unknown): '' | 'operator' | 'tuner' | 'vendor' {
  return v === 'vendor' || v === 'tuner' || v === 'operator' ? v : ''
}

export const auth = new AuthStore()

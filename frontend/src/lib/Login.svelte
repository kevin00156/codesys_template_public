<script lang="ts">
  import { auth } from './auth.svelte'
  import { t } from './i18n/store.svelte.ts'

  let password = $state('')
  let busy = $state(false)

  async function submit(e: Event) {
    e.preventDefault()
    if (busy || !password) return
    busy = true
    const ok = await auth.login(password)
    busy = false
    if (ok) password = ''
  }
</script>

<div class="login-wrap">
  <form class="login" onsubmit={submit}>
    <h2>🔒 {t('login.title')}</h2>
    <p class="hint">{t('login.hint')}</p>
    <input
      type="password"
      bind:value={password}
      placeholder={t('login.password')}
      autocomplete="current-password"
      disabled={busy}
    />
    <button type="submit" disabled={busy || !password}>
      {busy ? t('login.verifying') : t('login.submit')}
    </button>
    {#if auth.error}<p class="err">{auth.error}</p>{/if}
  </form>
</div>

<style>
  .login-wrap { display: flex; justify-content: center; padding: 3rem 1rem; }
  .login {
    display: flex; flex-direction: column; gap: 0.9rem;
    width: 100%; max-width: 320px;
    background: var(--c-panel); border: 1px solid var(--c-line);
    border-radius: var(--radius); padding: 1.6rem;
  }
  .login h2 { margin: 0; font-size: 1.15rem; color: var(--c-text); }
  .hint { margin: 0; font-size: 0.85rem; color: var(--c-muted); }
  .login input {
    min-height: var(--tap); padding: 0 0.8rem;
    background: var(--c-bg); color: var(--c-text);
    border: 1px solid var(--c-line); border-radius: var(--radius-sm);
    font-size: 1rem;
  }
  .login input:focus { outline: none; border-color: var(--c-accent); }
  .login button {
    min-height: var(--tap); border: none; border-radius: var(--radius-sm);
    background: var(--c-accent); color: #fff; font-size: 1rem; cursor: pointer;
  }
  .login button:hover:not(:disabled) { background: var(--c-accent-2); }
  .login button:disabled { opacity: 0.5; cursor: not-allowed; }
  .err {
    margin: 0; padding: 0.6rem 0.7rem; white-space: pre-wrap;
    background: var(--c-errbox-bg); color: var(--c-danger);
    border-radius: var(--radius-sm); font-size: 0.85rem;
  }
</style>

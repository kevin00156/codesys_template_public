// i18n 核心 store。前端第二個 localStorage 持久化項（第一個是 settings 機構尺寸）。
//
// 設計取捨（為何不上 svelte-i18n）：這是固定字串集的 kiosk HMI，沒有 ICU 複數 /
// 動態載入語系包的需求。自寫 rune store 與既有 settings/auth/nav 同一套寫法、
// 零依賴、無 async 載入閃爍（FOUC），且 t() 直接讀 $state.locale —— 在 template /
// $derived 中呼叫即具反應性，切語言全畫面即時更新，不需任何訂閱樣板。
//
// 資料結構：messages[locale][namespace][key]（見 messages/index.ts）。
// 查詢用 dot path：t('nav.machine') → messages[locale].nav.machine。

import { messages, type Locale } from './messages/index.ts'

export type { Locale }

const LS_KEY = 'ct.locale.v1' // 版號化 key：日後改 schema 可平滑遷移

function load(): Locale {
  try {
    const v = localStorage.getItem(LS_KEY)
    if (v === 'zh' || v === 'en') return v
  } catch {
    /* localStorage 不可用（隱私模式）→ 退回中文 */
  }
  return 'zh'
}

/** 依 dot path 取字串；任一層缺失或非字串都回 undefined（交由呼叫端退回）。 */
function resolve(dict: Record<string, unknown>, key: string): string | undefined {
  let cur: unknown = dict
  for (const part of key.split('.')) {
    if (cur == null || typeof cur !== 'object') return undefined
    cur = (cur as Record<string, unknown>)[part]
  }
  return typeof cur === 'string' ? cur : undefined
}

/** {name} 佔位替換；缺對應 param 時原樣保留（缺漏看得見，不靜默吞掉）。 */
function interpolate(s: string, params?: Record<string, string | number>): string {
  if (!params) return s
  return s.replace(/\{(\w+)\}/g, (m, k) => (k in params ? String(params[k]) : m))
}

class I18nStore {
  locale = $state<Locale>(load())

  setLocale(l: Locale) {
    this.locale = l
    try {
      localStorage.setItem(LS_KEY, l)
    } catch {
      /* localStorage 不可用 → 本次會話有效，重整後回預設 */
    }
    syncDocLang(l)
  }

  toggle() {
    this.setLocale(this.locale === 'zh' ? 'en' : 'zh')
  }

  /**
   * 翻譯查詢。讀 this.locale（$state）使呼叫處在反應上下文中追蹤語系，
   * 切語言即重算。查無 key 先退回中文（en 缺譯時不空白），再退回 key 本身
   * （連中文都沒有 → 直接顯示 key，缺譯一眼看得出來）。
   */
  t = (key: string, params?: Record<string, string | number>): string => {
    const s = resolve(messages[this.locale], key) ?? resolve(messages.zh, key) ?? key
    return interpolate(s, params)
  }
}

function syncDocLang(l: Locale) {
  try {
    document.documentElement.lang = l === 'zh' ? 'zh-Hant' : 'en'
  } catch {
    /* 無 document（測試環境）→ 略過 */
  }
}

export const i18n = new I18nStore()
/** 便捷別名：import { t } 即可；仍綁定實例，讀 locale $state 故保有反應性。 */
export const t = i18n.t

/** 語言切換器用：id + 顯示標籤（標籤本身不翻，永遠顯示母語名）。 */
export const LOCALES: { id: Locale; label: string }[] = [
  { id: 'zh', label: '中文' },
  { id: 'en', label: 'English' },
]

syncDocLang(i18n.locale) // 首次載入同步 <html lang>

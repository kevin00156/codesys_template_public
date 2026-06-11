// 訊息目錄組裝器。每個 namespace 一個檔，匯出 { zh:{...}, en:{...} };此處依
// namespace 名組成 messages[locale][namespace][key]。
//
// 為何拆檔：各畫面各佔一個 namespace 檔，互不踩線（可並行編輯）。新增 namespace
// 只動兩處 —— 新建檔 + 在下面 NS 表加一行。index 不放任何字串。
//
// 這是乾淨範本：只保留 common / nav / login / demo 四個機種無關的 namespace。
// 加新畫面時，在 messages/ 新增一個檔並在 NS 表登記。

export type Locale = 'zh' | 'en'

import { common } from './common.ts'
import { nav } from './nav.ts'
import { login } from './login.ts'
import { demo } from './demo.ts'

// 每個 namespace 的型別：{ zh: 巢狀字典, en: 巢狀字典 }。值用 unknown，查詢端
// （store.resolve）走 dot path 自行守門，故允許巢狀物件。
type Namespace = Record<Locale, Record<string, unknown>>

const NS: Record<string, Namespace> = {
  common,
  nav,
  login,
  demo,
}

function build(loc: Locale): Record<string, unknown> {
  const out: Record<string, unknown> = {}
  for (const name in NS) out[name] = NS[name][loc]
  return out
}

export const messages: Record<Locale, Record<string, unknown>> = {
  zh: build('zh'),
  en: build('en'),
}

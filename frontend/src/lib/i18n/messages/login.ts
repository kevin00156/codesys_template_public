// 登入頁（Login.svelte）。🔒 圖示留在 template，不進字串。
export const login = {
  zh: {
    title: '需要登入',
    hint: '操作機台需要登入；上方即時資料可直接檢視。',
    password: '密碼',
    verifying: '驗證中…',
    submit: '登入',
    // auth store 的前端後備錯誤（後端有給 error 訊息時優先用後端的）
    failed: '登入失敗 ({status})',
    networkError: '網路錯誤',
    // 範本內建預設密碼（後端未設任何 PLC_BRIDGE_*_HASH 時）
    defaultTitle: '範本預設密碼',
    defaultWarn: '這是範本內建的預設密碼，正式部署請務必變更。',
    defaultBanner: '⚠ 正在使用範本預設密碼（{pw}）— 正式部署請設定 PLC_BRIDGE_PASSWORD_HASH 變更，見登入頁說明。',
    // 如何變更密碼
    changeTitle: '如何變更密碼',
    changeIntro: '在部署主機產生雜湊、寫入環境變數檔後重啟服務：',
    changeCmd: `# 部署主機（WSL：/opt/plc_bridge/）
echo -n '你的新密碼' | ./plc_bridge -gen-hash
# 把輸出的 hash 寫進 plc_bridge.env：
#   PLC_BRIDGE_PASSWORD_HASH=<貼上 hash>
sudo systemctl restart plc_bridge`,
  },
  en: {
    title: 'Login required',
    hint: 'Operating the machine requires a login; the live data above is open.',
    password: 'Password',
    verifying: 'Verifying…',
    submit: 'Log in',
    failed: 'Login failed ({status})',
    networkError: 'Network error',
    defaultTitle: 'Template default password',
    defaultWarn: 'This is the template’s built-in default — change it before production.',
    defaultBanner: '⚠ Using the template default password ({pw}) — set PLC_BRIDGE_PASSWORD_HASH to change it before production; see the login screen.',
    changeTitle: 'How to change the password',
    changeIntro: 'On the deploy host, mint a hash, write it into the env file, then restart:',
    changeCmd: `# deploy host (WSL: /opt/plc_bridge/)
echo -n 'your-new-password' | ./plc_bridge -gen-hash
# write the printed hash into plc_bridge.env:
#   PLC_BRIDGE_PASSWORD_HASH=<paste hash>
sudo systemctl restart plc_bridge`,
  },
}

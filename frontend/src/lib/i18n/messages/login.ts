// 登入頁（Login.svelte）。🔒 圖示留在 template，不進字串。
export const login = {
  zh: {
    title: '需要登入',
    hint: '此頁面需要登入才能存取。',
    password: '密碼',
    verifying: '驗證中…',
    submit: '登入',
    // auth store 的前端後備錯誤（後端有給 error 訊息時優先用後端的）
    failed: '登入失敗 ({status})',
    networkError: '網路錯誤',
  },
  en: {
    title: 'Login required',
    hint: 'You must log in to access this page.',
    password: 'Password',
    verifying: 'Verifying…',
    submit: 'Log in',
    failed: 'Login failed ({status})',
    networkError: 'Network error',
  },
}

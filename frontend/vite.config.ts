import { defineConfig, loadEnv } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, '..', '')
  const plcHost = env.PLC_HOST || '192.168.1.10'
  return {
    plugins: [svelte()],
    build: {
      outDir: 'dist',
      emptyOutDir: true
    },
    server: {
      proxy: {
        '/ws': {
          // plc_bridge default HTTP/WS port is :8443 (see backend main.go).
          target: `ws://${plcHost}:8443`,
          ws: true,
          changeOrigin: true,
          // 後端 /ws 用同源檢查（CSWSH 防護）。經 dev proxy 時 Origin 是
          // localhost:5173、Host 是後端，必被拒；拿掉 Origin 走「非瀏覽器
          // 客戶端」路徑放行。正式版前端由後端同源服務，不經此路。
          configure: (proxy) => {
            proxy.on('proxyReqWs', (proxyReq) => proxyReq.removeHeader('origin'))
          }
        },
        '/api': {
          target: `http://${plcHost}:8443`,
          changeOrigin: true
        }
      }
    }
  }
})

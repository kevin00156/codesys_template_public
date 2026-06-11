import { test, expect } from '@playwright/test'

// Trivial smoke for the clean template: the page renders and the i18n language
// switch works end-to-end. No backend / PLC needed — the auth-status probe is
// mocked to "disabled" so the dashboard (not the login gate) shows, and the
// /ws upgrade is allowed to fail (the app just stays "Connecting…").
//
// This is the scaffold pattern; grow it into real screen tests as the template
// turns into a machine. Run with `npm run test:e2e` (after `npx playwright
// install chromium`).
test('loads and switches language', async ({ page }) => {
  await page.route('**/api/auth/status', (route) =>
    route.fulfill({ json: { enabled: false, loggedIn: true, role: '', operatorGated: false } }),
  )

  await page.goto('/')

  const title = page.locator('header .title')
  await expect(title).toHaveText('PLC 橋接器範本') // default locale zh

  await page.getByRole('button', { name: 'English' }).click()
  await expect(title).toHaveText('PLC Bridge Template') // re-translates live
})

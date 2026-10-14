import { test, expect } from '@playwright/test';
const profile = { id: 'profile-1', name: 'Общий', domain: 'node.test', certificate_id: 'cert-1', group_id: null, draft_revision: 7, published_revision: 2, document: { native_port: 8443, base_path: '', users: [{ username: 'alice', secret: '', enabled: true, quota_bytes: 0, up_bps: 0, down_bps: 0 }], settings: {} }, nodes: [] };
test('node wizard sends selected profile and produces a copyable Compose', async ({ page }) => {
  await page.route('**/api/**', async route => {
    const path = new URL(route.request().url()).pathname;
    const data = path === '/api/session' ? { user: { id: 'owner', username: 'owner', role: 'owner' }, csrf: 'csrf' } : path === '/api/profiles' ? [profile] : path === '/api/managed-nodes' ? { id: 'new-node', compose: 'services:\n  node:\n    image: ghcr.io/example/node@sha256:abc' } : [];
    await route.fulfill({ json: { ok: true, data } });
  });
  await page.goto('/'); await page.getByRole('navigation').getByRole('button', { name: 'Серверы', exact: true }).click();
  await page.getByRole('button', { name: 'Добавить сервер', exact: true }).click();
  await page.getByLabel('Название ноды', { exact: true }).fill('Node B'); await page.getByLabel('IP новой ноды', { exact: true }).fill('203.0.113.2');
  const request = page.waitForRequest(r => r.url().endsWith('/api/managed-nodes'));
  await page.getByRole('button', { name: 'Далее', exact: true }).click();
  expect((await request).postDataJSON()).toMatchObject({ name: 'Node B', ip: '203.0.113.2', profile_id: profile.id });
  await expect(page.getByRole('button', { name: 'Скопировать Compose', exact: true })).toBeVisible();
  await expect(page.locator('pre')).toContainText('services:');
});
test('profile conflict preserves edits and sends the displayed If-Match', async ({ page }) => {
  await page.route('**/api/**', async route => {
    const path = new URL(route.request().url()).pathname;
    if (route.request().method() === 'PATCH') { expect(route.request().headers()['if-match']).toBe('7'); await route.fulfill({ status: 409, json: { ok: false, error: { code: 'profile_revision_conflict' } } }); return; }
    const data = path === '/api/session' ? { user: { id: 'owner', username: 'owner', role: 'owner' }, csrf: 'csrf' } : path === '/api/profiles' ? [profile] : [];
    await route.fulfill({ json: { ok: true, data } });
  });
  await page.goto('/'); await page.getByRole('navigation').getByRole('button', { name: 'Общие профили', exact: true }).click();
  await page.getByRole('button', { name: 'Параметры и пользователи' }).click(); await page.getByLabel('Порт обычного MTProxy').fill('9443');
  await page.getByRole('button', { name: 'Сохранить черновик', exact: true }).click();
  await expect(page.getByRole('dialog')).toContainText('Профиль изменён другим администратором'); await expect(page.getByLabel('Порт обычного MTProxy')).toHaveValue('9443');
});

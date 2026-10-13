import { test, expect, type Page } from '@playwright/test';
const first = '11111111-1111-4111-8111-111111111111';
const second = '22222222-2222-4222-8222-222222222222';
async function fixture(page: Page, role: 'owner' | 'operator' | 'viewer') {
  const config = { shaping: { enabled: true, up_bps: 100000000, down_bps: 100000000, priority_percent: 25, bulk_enter_bytes: 262144, bulk_exit_bytes: 65536, max_ips: 32768 } };
  const nodes = [first, second].map((id, index) => ({ id, name: index ? 'Reserve' : 'Primary', api_url: `http://10.0.0.${index + 2}:9091/`, document: { ipv4: `192.0.2.${index + 1}`, ipv6: '', port: 443, sni: 'example.org', probe_dc: 2 }, enabled: true, collected_at: new Date().toISOString(), snapshot: { ready: true, management: 'reachable', upload_bps: 1000000, download_bps: 5000000, shaping: config.shaping } }));
  await page.route('**/api/**', async route => {
    const url = new URL(route.request().url()); let data: unknown = [];
    const path = url.pathname;
    if (path === '/api/session') data = { user: { id: first, username: 'fixture', role }, csrf: 'fixture-csrf' };
    if (path === '/api/nodes') data = nodes;
    if (path === '/api/operations') data = route.request().method() === 'POST' ? { id: 'operation-id', state: 'queued' } : [{ id: 'operation-id', state: 'unknown', results: [{ node_id: first, state: 'unknown' }], created_at: new Date().toISOString(), updated_at: new Date().toISOString() }];
    if (path.endsWith('/history')) data = [];
    if (path.endsWith('/read')) {
      const target = url.searchParams.get('path') ?? '';
      let value: unknown = {};
      if (target.startsWith('/v1/config')) value = config;
      if (target.startsWith('/v1/runtime/shaping')) value = { enabled: true, policy: config.shaping, retained_ips: 1, throttle_total: 8, admission_rejected_total: 0, ips: [{ ip: '192.0.2.100', window_bytes: [1024, 4096], speed_bps: [1000, 1000], epoch_bytes: [[100, 100], [100, 100]], flows: 4, bulk_flows: [0, 1] }] };
      if (target === '/v1/runtime/interfaces') value = { addresses: [{ ip: '192.0.2.1', interface: 'eth0', usable: true }, { ip: '192.0.2.3', interface: 'eth0', usable: true }, { ip: '127.0.0.1', interface: 'lo', usable: false }], active_listeners: [{ ip: '192.0.2.1', port: 443, transport: 'mtproxy' }] };
      if (target === '/v1/runtime/web/status') value = { available: true, runtime: { runtime_instance: '0123456789abcdef0123456789abcdef' } };
      if (target === '/v1/runtime/web/sessions') value = { sessions: [] };
      if (target === '/v1/users') value = [{ username: 'alice', enabled: true, in_runtime: true, current_connections: 4, active_unique_ips: 1, total_octets: 1048576, ...(role !== 'viewer' ? { links: { tls: ['tg://proxy?server=proxy.test&port=443&secret=eeee'] } } : {}) }];
      data = { status: 200, response: { ok: true, data: value, revision: 'revision-fixture' } };
    }
    await route.fulfill({ json: { ok: true, data }, headers: { 'Cache-Control': 'no-store' } });
  });
  await page.goto('/'); await expect(page.getByRole('heading', { name: 'Overview', exact: true })).toBeVisible();
}

test('owner dashboard and all sections render without browser errors', async ({ page }) => {
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  await fixture(page, 'owner');
  await expect(page.getByText('2 / 20')).toBeVisible();
  for (const name of ['Servers', 'Users', 'Configuration', 'Shaping', 'Diagnostics', 'External probes', 'DNS failover', 'IP rotation', 'Operations', 'Audit', 'Administrators']) {
    await page.getByRole('navigation').getByRole('button', { name, exact: true }).click(); await expect(page.getByRole('heading', { name, exact: true })).toBeVisible();
  }
  expect(errors).toEqual([]);
  await page.getByRole('navigation').getByRole('button', { name: 'Overview', exact: true }).click();
  await page.screenshot({ path: '/private/tmp/telemt-console-desktop.png', fullPage: true });
});

test('viewer has no write controls or secret links', async ({ page }) => {
  await fixture(page, 'viewer');
  await expect(page.getByRole('button', { name: 'Administrators', exact: true })).toHaveCount(0);
  await expect(page.getByText('Mutation targets')).toHaveCount(0);
  await page.getByRole('navigation').getByRole('button', { name: 'Users', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Create user' })).toHaveCount(0);
  await page.getByRole('button', { name: 'Details', exact: true }).click();
  await expect(page.locator('a[href^="tg://"]')).toHaveCount(0);
  await expect(page.getByRole('dialog').getByRole('button', { name: 'Save', exact: true })).toBeDisabled();
  await page.keyboard.press('Escape'); await expect(page.getByRole('dialog')).toHaveCount(0);
});

test('shaping writes contain revision without connection churn, never an optimistic success', async ({ page }) => {
  await fixture(page, 'operator');
  await page.getByRole('navigation').getByRole('button', { name: 'Shaping', exact: true }).click();
  await page.getByLabel('Usable download channel (bits/s)').fill('50000000');
  const request = page.waitForRequest(request => request.url().endsWith('/api/operations') && request.method() === 'POST');
  await page.getByRole('button', { name: 'Apply shaping policy' }).click();
  const body = (await request).postDataJSON();
  expect(body.revisions[first]).toBe('revision-fixture'); expect(body.path).toBe('/v1/config'); expect(body.body.shaping.down_bps).toBe(50000000);
  await expect(page.getByRole('status')).toContainText('submitted');
  await expect(page.getByRole('status')).not.toContainText('succeeded');
});

test('mobile cabinet stays inside viewport', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 }); await fixture(page, 'owner');
  await page.getByRole('navigation').getByRole('button', { name: 'Servers', exact: true }).click();
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await page.screenshot({ path: '/private/tmp/telemt-console-mobile.png', fullPage: true });
});

test('shaping form retains its displayed revision across later configuration changes', async ({ page }) => {
  await fixture(page, 'operator'); await page.getByRole('navigation').getByRole('button', { name: 'Shaping', exact: true }).click();
  await expect(page.getByText('Saved revision: revision-fixture')).toBeVisible();
  await page.route('**/api/nodes/*/read*', async route => {
    if (new URL(route.request().url()).searchParams.get('path') === '/v1/config') await route.fulfill({ json: { ok: true, data: { status: 200, response: { ok: true, data: { shaping: { enabled: false } }, revision: 'newer-revision' } } } });
    else await route.fallback();
  });
  const sent = page.waitForRequest(request => request.url().endsWith('/api/operations') && request.method() === 'POST');
  await page.getByRole('button', { name: 'Apply shaping policy' }).click(); expect((await sent).postDataJSON().revisions[first]).toBe('revision-fixture');
});

test('WEB commands use the inspected process identity and one server target', async ({ page }) => {
  await fixture(page, 'operator'); await page.getByLabel('Reserve', { exact: true }).check();
  await page.getByRole('navigation').getByRole('button', { name: 'Diagnostics', exact: true }).click(); await page.getByRole('button', { name: 'WEB status', exact: true }).click();
  const sent = page.waitForRequest(request => request.url().endsWith('/api/operations') && request.method() === 'POST');
  await page.getByRole('button', { name: 'Pause new WEB sessions' }).click(); const body = (await sent).postDataJSON();
  expect(body.node_ids).toEqual([first]); expect(body.body.runtime_instance).toBe('0123456789abcdef0123456789abcdef'); expect(body.revisions[first]).toBe('revision-fixture');
  await expect(page.getByText('Queued operation: operation-id. Completion is tracked in Operations.')).toBeVisible();
});


test('assigned IP pool creation defaults off and cannot select a local unusable address', async ({ page }) => {
  await fixture(page, 'owner');
  const group = { id: second, name: 'Single-server DNS', active_node: first, automatic: false, status: 'disabled', document: { node_ids: [first] }, observed: {} };
  await page.route('**/api/groups', route => route.fulfill({ json: { ok: true, data: [group] } }));
  let created = false;
  await page.route('**/api/ip-pools', async route => {
    if (route.request().method() === 'POST') { created = true; await route.fulfill({ json: { ok: true, data: { id: second } } }); }
    else await route.fulfill({ json: { ok: true, data: created ? [{ id: second, name: 'Assigned pool', node_id: first, group_id: second, addresses: ['192.0.2.1', '192.0.2.3'], automatic: false, status: 'unprepared' }] : [] } });
  });
  await page.getByRole('button', { name: 'Refresh', exact: true }).click();
  await page.getByRole('navigation').getByRole('button', { name: 'IP rotation', exact: true }).click();
  await page.getByRole('button', { name: 'Create IP pool', exact: true }).click();
  const dialog = page.getByRole('dialog');
  await dialog.getByLabel('Pool name').fill('Assigned pool');
  await dialog.getByLabel('Enrolled DNS group').selectOption(second);
  await expect(dialog.getByLabel('127.0.0.1 · lo · unavailable')).toBeDisabled();
  await dialog.getByLabel('192.0.2.1 · eth0').check(); await dialog.getByLabel('192.0.2.3 · eth0').check();
  const pending = page.waitForRequest(request => request.url().endsWith('/api/ip-pools') && request.method() === 'POST');
  await dialog.getByRole('button', { name: 'Create pool (auto rotation off)' }).click();
  expect((await pending).postDataJSON()).toEqual({ name: 'Assigned pool', node_id: first, group_id: second, addresses: ['192.0.2.1', '192.0.2.3'] });
  await expect(page.getByRole('button', { name: 'Enable auto rotation' })).toBeVisible();
  await page.getByRole('button', { name: 'Inspect pool' }).click();
  await expect(page.getByRole('dialog').getByText('Automatic rotation: disabled.', { exact: false })).toBeVisible();
});

test('bulk caps are independently configurable with the displayed revision', async ({ page }) => {
  await fixture(page, 'operator');
  await page.getByRole('navigation').getByRole('button', { name: 'Shaping', exact: true }).click();
  await page.getByLabel('Bulk upload cap under contention (bits/s; 0 = automatic)', { exact: true }).fill('10000000');
  await page.getByLabel('Bulk download cap under contention (bits/s; 0 = automatic)', { exact: true }).fill('20000000');
  const sent = page.waitForRequest(request => request.url().endsWith('/api/operations') && request.method() === 'POST');
  await page.getByRole('button', { name: 'Apply shaping policy' }).click();
  const body = (await sent).postDataJSON();
  expect(body.revisions[first]).toBe('revision-fixture');
  expect(body.body.shaping.bulk_up_bps).toBe(10000000);
  expect(body.body.shaping.bulk_down_bps).toBe(20000000);
});

test('owner can submit a public hostname as the Control API URL', async ({ page }) => {
  await fixture(page, 'owner');
  await page.getByRole('navigation').getByRole('button', { name: 'Servers', exact: true }).click();
  await page.getByRole('button', { name: 'Add server', exact: true }).click();
  await page.getByLabel('Server name', { exact: true }).fill('Public node');
  await page.getByLabel('Control API URL', { exact: true }).fill('https://node-api.example.org:9443/');
  await page.getByLabel('FakeTLS domain (SNI)', { exact: true }).fill('example.org');
  const sent = page.waitForRequest(request => request.url().endsWith('/api/nodes') && request.method() === 'POST');
  await page.getByRole('dialog').getByRole('button', { name: 'Save', exact: true }).click();
  expect((await sent).postDataJSON().api_url).toBe('https://node-api.example.org:9443/');
});

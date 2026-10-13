import { test, expect, type Page } from '@playwright/test';
const id = '11111111-1111-4111-8111-111111111111';
const reserve = '22222222-2222-4222-8222-222222222222';
async function setup(page: Page, missing = false) {
  const now = Date.now();
  const snapshot = {
    management: 'reachable', ready: true, upload_bps: 1000000, download_bps: 150000000,
    traffic: { available: true, active_users: 15, current_connections: 180, octets_from_client: 1024 ** 3, octets_to_client: 3 * 1024 ** 3 },
    info: { version: '3.5.9', uptime_seconds: 22000, process_started_at_epoch_secs: Math.floor(now / 1000) - 22000 },
    summary: { connections_total: 10000, connections_bad_total: 1000, handshake_timeouts_total: 50, connections_bad_by_class: [{ class: 'tls_handshake_bad_client', total: 1000 }], handshake_failures_by_class: [] },
    handshake_timeouts_per_minute: 12,
    host: { available: true, cpu_model: 'Ryzen 9 7950X', cpu: { cores: 4 }, cpu_idle_percent: 9, cpu_busy_percent: 91, memory_total_bytes: 4 * 1024 ** 3, memory_available_bytes: 1024 ** 3, swap_total_bytes: 1024 ** 3, swap_free_bytes: 1024 ** 3, process_rss_bytes: 2 * 1024 ** 3, process_cpu_cores: 1.7, uptime_seconds: 23000, load1: 2.35, softirq_percent: 39, sockets: 29908, tcp: { CurrEstab: 10000 }, conntrack_count: 65000, conntrack_max: 524000, tcp_retransmit_percent: 2.4, disk: { total_bytes: 1000000, available_bytes: 800000 } },
    shaping: { enabled: true, waiting_users: 3, policy: { up_bps: 200000000, down_bps: 200000000, priority_percent: 25, bulk_up_bps: 40000000, bulk_down_bps: 40000000, bulk_idle_up_bps: 140000000, bulk_idle_down_bps: 140000000 } },
    dcs: { middle_proxy_enabled: true, generated_at_epoch_secs: now / 1000, dcs: [{ dc: -2, endpoints: ['192.0.2.1:443'], available_endpoints: 1, available_pct: 100, required_writers: 5, alive_writers: 5, rtt_ms: 15, load: 180 }] }, gates: { route_mode: 'middle' },
  };
  const nodes = [{ id, name: 'one-156.122', enabled: true, collected_at: new Date(now - (missing ? 60000 : 0)).toISOString(), document: { ipv4: '89.125.156.122' }, snapshot: missing ? { management: 'reachable', ready: true } : snapshot }, { id: reserve, name: 'Резерв', enabled: true, collected_at: new Date(now - 60000).toISOString(), document: { ipv4: '89.125.156.183' }, snapshot: {} }];
  let shapingReads = 0;
  await page.route('**/api/**', async route => {
    const url = new URL(route.request().url()); let data: unknown = [];
    if (url.pathname === '/api/session') data = { user: { id, username: 'reader', role: 'viewer' }, csrf: 'fixture' };
    if (url.pathname === '/api/nodes') data = nodes;
    if (url.pathname.endsWith('/connections-map')) data = { available: true, generated_at: new Date().toISOString(), database: { status: 'ready', build_epoch: 100 }, ips: [{ ip: '2001:db8:abcd:ffff:1234:5678:90ab:cdef', connections: 2, users: 1, location: { latitude: 55, longitude: 37, city: 'Москва', country: 'Россия' } }], endpoints: [], total_ips: 1, connections: 2, active_users: 1, cleanup_pending: 0, truncated: false, located_ips: 1, non_public_ips: 0 };
    if (url.pathname.endsWith('/read') && url.searchParams.get('path')?.startsWith('/v1/runtime/shaping')) {
      shapingReads++;
      data = { status: 200, response: { ok: true, revision: 'fixture', data: { enabled: true, retained_ips: 1, ips: [{ ip: '2001:db8:abcd:ffff:1234:5678:90ab:cdef', speed_bps: [1000000, 27000000], window_bytes: [1024, 1048576] }] } } };
    }
    await route.fulfill({ json: { ok: true, data } });
  });
  await page.goto('/');
  return () => shapingReads;
}

test('server overview shows real metrics, alerts and bounded IP drilldown on desktop and mobile', async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 1440, height: 1100 });
  await page.emulateMedia({ colorScheme: 'dark' });
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  const reads = await setup(page);
  const card = page.getByRole('article', { name: 'Сводка сервера one-156.122', exact: true });
  await expect(card.getByText('Ryzen 9 7950X')).toBeVisible();
  await expect(page.getByRole('button', { name: /CPU свободно 9%/ })).toBeVisible();
  await expect(card.getByLabel('ME: доступен', { exact: true })).toBeVisible();
  await expect(card.getByText('140 Мбит/с', { exact: true })).toHaveCount(2);
  await expect(card.getByText('15 мс', { exact: true })).toBeVisible();
  expect(reads()).toBe(0);
  await card.getByText('Причины ошибок · с запуска процесса', { exact: true }).click();
  await expect(card.getByText('tls_handshake_bad_client', { exact: true })).toBeVisible();
  await page.evaluate(() => window.scrollTo(0, 0));
  await page.screenshot({ path: testInfo.outputPath('telemt-overview-desktop.png'), fullPage: true });
  await page.setViewportSize({ width: 390, height: 844 });
  await card.getByRole('button', { name: 'Показать IP клиентов', exact: true }).click();
  await expect(card.getByText('28 Мбит/с', { exact: true })).toBeVisible();
  await expect(card.getByText('Москва, Россия', { exact: true })).toBeVisible();
  expect(reads()).toBeGreaterThan(0);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await card.screenshot({ path: testInfo.outputPath('telemt-overview-mobile.png'), style: '.app-header, .mobile-nav { visibility: hidden !important; }' });
  await page.getByRole('article', { name: 'Сводка сервера Резерв', exact: true }).getByRole('button', { name: 'Карта и графики', exact: true }).click();
  await expect(page.getByLabel('Просматриваемый сервер', { exact: true })).toHaveValue(reserve);
  expect(errors).toEqual([]);
});

test('stale and unavailable overview values never appear as healthy or zero', async ({ page }) => {
  await setup(page, true);
  const card = page.getByRole('article', { name: 'Сводка сервера one-156.122', exact: true });
  await expect(card.getByLabel('API: нет актуальных данных', { exact: true })).toBeVisible();
  await expect(card.getByLabel('ME: нет актуальных данных', { exact: true })).toBeVisible();
  await expect(card.getByText('Метрики ОС недоступны.', { exact: false })).toBeVisible();
  await expect(card.getByText('Нет данных', { exact: true }).first()).toBeVisible();
  await expect(card.getByText('0 Мбит/с', { exact: true })).toHaveCount(0);
  await expect(page.getByRole('button', { name: /Нет свежих измерений более 15 секунд/ })).toHaveCount(2);
});

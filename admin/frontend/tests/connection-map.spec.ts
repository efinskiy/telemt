import { test, expect, type Page } from '@playwright/test';
const first = '11111111-1111-4111-8111-111111111111', second = '22222222-2222-4222-8222-222222222222';
const moscow = { latitude: 55.75, longitude: 37.62, city: 'Москва', country: 'Россия' };
const berlin = { latitude: 52.52, longitude: 13.4, city: 'Берлин', country: 'Германия' };
function snapshot() { return { available: true, generated_at: new Date().toISOString(), database: { status: 'ready', build_epoch: 1790812800 }, connections: 7, active_users: 3, total_ips: 3, cleanup_pending: 0, truncated: false, located_ips: 2, non_public_ips: 1, ips: [{ ip: '8.8.8.8', connections: 4, users: 2, location: moscow }, { ip: '1.1.1.1', connections: 2, users: 1, location: berlin }, { ip: '10.0.0.1', connections: 1, users: 1, location: null }], endpoints: [{ ip: '9.9.9.9', location: { latitude: 60.17, longitude: 24.94, city: 'Хельсинки', country: 'Финляндия' } }] }; }
async function fixture(page: Page, map: (node: string) => unknown = () => snapshot()) {
  const nodes = [first, second].map((id, index) => ({ id, name: index ? 'Резерв' : 'Основной', enabled: true, api_url: 'https://node.test', document: { ipv4: '9.9.9.9' }, snapshot: { management: 'reachable', ready: true }, collected_at: new Date().toISOString() }));
  await page.route('**/api/**', async route => {
    const path = new URL(route.request().url()).pathname;
    let data: unknown = [];
    if (path === '/api/session') data = { user: { id: first, username: 'reader', role: 'viewer' }, csrf: 'test' };
    if (path === '/api/nodes') data = nodes;
    if (path.endsWith('/connections-map')) {
      const value = map(path.split('/')[3]);
      if (value === 'fail') { await route.fulfill({ status: 502, json: { ok: false, error: { code: 'map_node_unavailable' } } }); return; }
      data = value;
    }
    await route.fulfill({ json: { ok: true, data } });
  });
  await page.goto('/'); await expect(page.getByRole('heading', { name: 'Подключения на карте · Основной' })).toBeVisible();
}

test('live globe supports projection, zoom, selection and IP search without third-party requests', async ({ page }) => {
  const errors: string[] = [], external: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('request', request => { if (!request.url().startsWith('http://127.0.0.1:5178')) external.push(request.url()); });
  await fixture(page);
  const surface = page.getByRole('group', { name: 'Интерактивная карта активных подключений' });
  await expect(page.locator('[data-map-point]')).toHaveCount(2);
  await expect(page.locator('.map-route')).toHaveCount(2);
  await expect(page.locator('.map-metrics')).toContainText('Пользователи онлайн');
  await page.locator('.connections-map').screenshot({ path: '/private/tmp/telemt-connections-globe.png' });
  const before = await surface.locator('path').nth(2).getAttribute('d');
  await page.getByRole('button', { name: 'Увеличить карту' }).click();
  expect(await surface.locator('path').nth(2).getAttribute('d')).not.toBe(before);
  await page.getByRole('button', { name: 'Карта', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Карта', exact: true })).toHaveAttribute('aria-pressed', 'true');
  await page.getByRole('complementary', { name: 'Регионы подключений' }).getByRole('button').first().click();
  await expect(page.getByRole('heading', { name: 'Подключения · Москва, Россия' })).toBeVisible();
  await expect(page.locator('.connections-map tbody tr')).toHaveCount(1);
  await page.getByRole('button', { name: 'Все подключения', exact: true }).click();
  await page.getByRole('textbox', { name: 'Поиск подключений' }).fill('10.0.0.1');
  await expect(page.locator('.connections-map tbody tr')).toHaveCount(1);
  await expect(page.locator('.connections-map tbody')).toContainText('Координаты неизвестны');
  await page.getByRole('textbox', { name: 'Поиск подключений' }).fill('');
  await page.screenshot({ path: '/private/tmp/telemt-connections-map-desktop.png', fullPage: true });
  expect(errors).toEqual([]); expect(external).toEqual([]);
});

test('failed refresh marks old snapshot stale and changing nodes clears it', async ({ page }) => {
  let fail = false;
  await fixture(page, id => id === second ? { available: false, reason: 'node_update_required' } : fail ? 'fail' : snapshot());
  await expect(page.locator('.connections-map tbody')).toContainText('8.8.8.8');
  fail = true;
  await expect(page.getByText('Данные устарели', { exact: true })).toBeVisible({ timeout: 10000 });
  await expect(page.locator('.connections-map tbody')).toContainText('8.8.8.8');
  await page.getByRole('combobox', { name: 'Просматриваемый сервер' }).selectOption(second);
  await expect(page.getByText(/Обновите telemt на ноде/)).toBeVisible();
  await expect(page.locator('.connections-map tbody tr')).toHaveCount(0);
  await expect(page.locator('[data-map-point]')).toHaveCount(0);
});

test('missing geolocation keeps counters and unknown IP table', async ({ page }) => {
  const data = snapshot(); data.database.status = 'missing'; data.located_ips = 0;
  const response = { ...data, ips: data.ips.map(row => ({ ...row, location: null })), endpoints: data.endpoints.map(row => ({ ...row, location: null })) };
  await fixture(page, () => response);
  await expect(page.getByText(/База геолокации не установлена/)).toBeVisible();
  await expect(page.locator('.connections-map tbody tr')).toHaveCount(3);
  await expect(page.locator('[data-map-point]')).toHaveCount(0);
  await expect(page.locator('.map-route')).toHaveCount(0);
});

test('mobile map fits screen and geographic sidebar stays inside the map section', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 }); await fixture(page);
  await expect(page.locator('[data-map-point]')).toHaveCount(2);
  const position = await page.locator('.map-regions').evaluate(element => getComputedStyle(element).position);
  expect(position).not.toBe('fixed');
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await page.screenshot({ path: '/private/tmp/telemt-connections-map-mobile.png', fullPage: true });
});

test('truncated and deferred-cleanup snapshots have explicit notices and bounded map points', async ({ page }) => {
  const data = snapshot();
  const ips = Array.from({ length: 10000 }, (_, i) => ({ ip: `11.${Math.floor(i / 256)}.${i % 256}.1`, connections: 1, users: 1, location: { ...moscow, latitude: i % 160 - 80, longitude: i % 350 - 175 } }));
  await fixture(page, () => ({ ...data, ips, located_ips: 10000, total_ips: 11000, connections: 11000, active_users: 10000, truncated: true, cleanup_pending: 3, non_public_ips: 0 }));
  await expect(page.getByText(/Неполный снимок/)).toBeVisible();
  await expect(page.getByText(/событий отключения/)).toBeVisible();
  await expect(page.getByText(/самых активных групп/)).toBeVisible();
  expect(await page.locator('[data-map-point]').count()).toBeLessThanOrEqual(300);
  await expect(page.locator('.connections-map tbody tr')).toHaveCount(25);
});

test('leaving overview stops polling and empty live snapshot is not an error', async ({ page }) => {
  let requests = 0;
  await fixture(page, () => { requests++; return { ...snapshot(), ips: [], connections: 0, total_ips: 0, located_ips: 0, non_public_ips: 0, active_users: 0 }; });
  await expect(page.getByText('Сейчас нет активных подключений.')).toBeVisible();
  await page.getByRole('navigation').getByRole('button', { name: 'Операции', exact: true }).click();
  const count = requests;
  await page.waitForTimeout(5500);
  expect(requests).toBe(count);
});


test('globe can be dragged, rotated by keyboard and expanded with reduced motion', async ({ page }) => {
  await page.emulateMedia({ reducedMotion: 'reduce' }); await fixture(page);
  const surface = page.getByRole('group', { name: 'Интерактивная карта активных подключений' });
  await expect(page.locator('[data-map-point]')).toHaveCount(2);
  const geometry = surface.locator('path').nth(2);
  const before = await geometry.getAttribute('d');
  await surface.focus(); await page.keyboard.press('ArrowRight');
  expect(await geometry.getAttribute('d')).not.toBe(before);
  const box = await surface.boundingBox(); expect(box).not.toBeNull();
  if (!box) throw new Error('Map bounds unavailable');
  const moved = await geometry.getAttribute('d');
  await page.mouse.move(box.x + box.width * .3, box.y + box.height * .6);
  await page.mouse.down(); await page.mouse.move(box.x + box.width * .5, box.y + box.height * .6, { steps: 5 }); await page.mouse.up();
  expect(await geometry.getAttribute('d')).not.toBe(moved);
  await page.locator('.connections-map').screenshot({ path: '/private/tmp/telemt-connections-globe.png' });
  await page.getByRole('button', { name: 'На весь экран' }).click();
  await expect(page.locator('.connection-globe')).toHaveClass(/is-fullscreen/);
  await page.getByRole('button', { name: 'Свернуть', exact: true }).click();
  await expect(page.locator('.connection-globe')).not.toHaveClass(/is-fullscreen/);
  expect(await page.locator('.map-route').first().evaluate(element => getComputedStyle(element).animationName)).toBe('none');
});

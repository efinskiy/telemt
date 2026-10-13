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
  await page.goto('/'); await expect(page.getByRole('heading', { name: 'Обзор', exact: true })).toBeVisible();
}

test('owner dashboard and all sections render without browser errors', async ({ page }) => {
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  await fixture(page, 'owner');
  await expect(page.getByText('2 / 20')).toBeVisible();
  for (const name of ['Серверы', 'Пользователи', 'Конфигурация', 'Шейпер', 'Диагностика', 'Внешние проверки', 'Переключение DNS', 'Ротация IP', 'Операции', 'Аудит', 'Администраторы']) {
    await page.getByRole('navigation').getByRole('button', { name, exact: true }).click(); await expect(page.getByRole('heading', { name, exact: true })).toBeVisible();
  }
  expect(errors).toEqual([]);
  await page.getByRole('navigation').getByRole('button', { name: 'Обзор', exact: true }).click();
  await page.screenshot({ path: '/private/tmp/telemt-console-desktop.png', fullPage: true });
});

test('viewer has no write controls or secret links', async ({ page }) => {
  await fixture(page, 'viewer');
  await expect(page.getByRole('button', { name: 'Администраторы', exact: true })).toHaveCount(0);
  await expect(page.getByText('Серверы для изменений')).toHaveCount(0);
  await page.getByRole('navigation').getByRole('button', { name: 'Пользователи', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Создать пользователя' })).toHaveCount(0);
  await page.getByRole('button', { name: 'Подробнее', exact: true }).click();
  await expect(page.locator('a[href^="tg://"]')).toHaveCount(0);
  await expect(page.getByRole('dialog').getByRole('button', { name: 'Сохранить', exact: true })).toBeDisabled();
  await page.keyboard.press('Escape'); await expect(page.getByRole('dialog')).toHaveCount(0);
});

test('shaping writes contain revision without connection churn, never an optimistic success', async ({ page }) => {
  await fixture(page, 'operator');
  await page.getByRole('navigation').getByRole('button', { name: 'Шейпер', exact: true }).click();
  await page.getByLabel('Доступный канал к клиентам (бит/с)').fill('50000000');
  const request = page.waitForRequest(request => request.url().endsWith('/api/operations') && request.method() === 'POST');
  await page.getByRole('button', { name: 'Применить политику шейпера' }).click();
  const body = (await request).postDataJSON();
  expect(body.revisions[first]).toBe('revision-fixture'); expect(body.path).toBe('/v1/config'); expect(body.body.shaping.down_bps).toBe(50000000);
  await expect(page.getByRole('status')).toContainText('поставлена в очередь');
  await expect(page.getByRole('status')).not.toContainText('Завершено');
});

test('mobile cabinet stays inside viewport', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 }); await fixture(page, 'owner');
  await page.getByRole('navigation').getByRole('button', { name: 'Серверы', exact: true }).click();
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await page.screenshot({ path: '/private/tmp/telemt-console-mobile.png', fullPage: true });
});

test('shaping form retains its displayed revision across later configuration changes', async ({ page }) => {
  await fixture(page, 'operator'); await page.getByRole('navigation').getByRole('button', { name: 'Шейпер', exact: true }).click();
  await expect(page.getByText('Сохранённая ревизия: revision-fixture')).toBeVisible();
  await page.route('**/api/nodes/*/read*', async route => {
    if (new URL(route.request().url()).searchParams.get('path') === '/v1/config') await route.fulfill({ json: { ok: true, data: { status: 200, response: { ok: true, data: { shaping: { enabled: false } }, revision: 'newer-revision' } } } });
    else await route.fallback();
  });
  const sent = page.waitForRequest(request => request.url().endsWith('/api/operations') && request.method() === 'POST');
  await page.getByRole('button', { name: 'Применить политику шейпера' }).click(); expect((await sent).postDataJSON().revisions[first]).toBe('revision-fixture');
});

test('WEB commands use the inspected process identity and one server target', async ({ page }) => {
  await fixture(page, 'operator'); await page.getByLabel('Reserve', { exact: true }).check();
  await page.getByRole('navigation').getByRole('button', { name: 'Диагностика', exact: true }).click(); await page.getByRole('button', { name: 'Состояние WEB', exact: true }).click();
  const sent = page.waitForRequest(request => request.url().endsWith('/api/operations') && request.method() === 'POST');
  await page.getByRole('button', { name: 'Приостановить новые WEB-сессии' }).click(); const body = (await sent).postDataJSON();
  expect(body.node_ids).toEqual([first]); expect(body.body.runtime_instance).toBe('0123456789abcdef0123456789abcdef'); expect(body.revisions[first]).toBe('revision-fixture');
  await expect(page.getByText('Операция operation-id поставлена в очередь. Завершение отслеживается в разделе «Операции».')).toBeVisible();
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
  await page.getByRole('button', { name: 'Обновить', exact: true }).click();
  await page.getByRole('navigation').getByRole('button', { name: 'Ротация IP', exact: true }).click();
  await page.getByRole('button', { name: 'Создать пул IP', exact: true }).click();
  const dialog = page.getByRole('dialog');
  await dialog.getByLabel('Название пула').fill('Assigned pool');
  await dialog.getByLabel('Подключённая группа DNS').selectOption(second);
  await expect(dialog.getByLabel('127.0.0.1 · lo · недоступен')).toBeDisabled();
  await dialog.getByLabel('192.0.2.1 · eth0').check(); await dialog.getByLabel('192.0.2.3 · eth0').check();
  const pending = page.waitForRequest(request => request.url().endsWith('/api/ip-pools') && request.method() === 'POST');
  await dialog.getByRole('button', { name: 'Создать пул (авторотация выключена)' }).click();
  expect((await pending).postDataJSON()).toEqual({ name: 'Assigned pool', node_id: first, group_id: second, addresses: ['192.0.2.1', '192.0.2.3'] });
  await expect(page.getByRole('button', { name: 'Включить авторотацию' })).toBeVisible();
  await page.getByRole('button', { name: 'Открыть пул' }).click();
  await expect(page.getByRole('dialog').getByText('Авторотация: выключена.', { exact: false })).toBeVisible();
});

test('bulk caps are independently configurable with the displayed revision', async ({ page }) => {
  await fixture(page, 'operator');
  await page.getByRole('navigation').getByRole('button', { name: 'Шейпер', exact: true }).click();
  await page.getByLabel('Бюджет объёмной отправки при конкуренции (бит/с)', { exact: true }).fill('10000000');
  await page.getByLabel('Бюджет объёмной загрузки при конкуренции (бит/с)', { exact: true }).fill('20000000');
  const sent = page.waitForRequest(request => request.url().endsWith('/api/operations') && request.method() === 'POST');
  await page.getByRole('button', { name: 'Применить политику шейпера' }).click();
  const body = (await sent).postDataJSON();
  expect(body.revisions[first]).toBe('revision-fixture');
  expect(body.body.shaping.bulk_up_bps).toBe(10000000);
  expect(body.body.shaping.bulk_down_bps).toBe(20000000);
});

test('owner can submit a public hostname as the Control API URL', async ({ page }) => {
  await fixture(page, 'owner');
  await page.getByRole('navigation').getByRole('button', { name: 'Серверы', exact: true }).click();
  await page.getByRole('button', { name: 'Добавить сервер', exact: true }).click();
  await page.getByLabel('Название сервера', { exact: true }).fill('Public node');
  await page.getByLabel('Адрес API управления', { exact: true }).fill('https://node-api.example.org:9443/');
  await page.getByLabel('Домен FakeTLS (SNI)', { exact: true }).fill('example.org');
  const sent = page.waitForRequest(request => request.url().endsWith('/api/nodes') && request.method() === 'POST');
  await page.getByRole('dialog').getByRole('button', { name: 'Сохранить', exact: true }).click();
  expect((await sent).postDataJSON().api_url).toBe('https://node-api.example.org:9443/');
});

test('Users identifies a Fleet HTML gateway error and recovers after the route is restored', async ({ page }) => {
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  await fixture(page, 'owner');
  await page.route('**/api/nodes/*/read*', route => route.fulfill({ status: 502, contentType: 'text/html', body: '<!DOCTYPE html><p>private-response-detail</p>' }));
  await page.getByRole('navigation').getByRole('button', { name: 'Пользователи', exact: true }).click();
  await expect(page.locator('.error')).toContainText('API Fleet вернул ответ не в формате JSON (HTTP 502)');
  await expect(page.locator('.error')).toContainText('обратный прокси');
  await expect(page.getByRole('button', { name: 'Создать пользователя', exact: true })).toBeDisabled();
  await expect(page.getByText('private-response-detail')).toHaveCount(0);
  await page.unroute('**/api/nodes/*/read*');
  await expect(page.getByRole('button', { name: 'Создать пользователя', exact: true })).toBeEnabled({ timeout: 10000 });
  await expect(page.locator('.error')).toHaveCount(0);
  expect(errors).toEqual([]);
});

test('field configuration submits a sparse patch and retains the draft on conflict', async ({ page }) => {
  await fixture(page, 'operator');
  await page.route('**/api/nodes/*/read*', async route => {
    if (new URL(route.request().url()).searchParams.get('path') === '/v1/config') await route.fulfill({ json: {ok:true,data:{status:200,response:{ok:true,revision:'config-before',data:{general:{log_level:'normal',me_pool_min_fresh_ratio:1,telemetry:{user_enabled:false}},server:{listeners:[]},censorship:{tls_fetch:{profiles:[]}},web:{carriers:false,vhosts:[]}}}}} }); else await route.fallback();
  });
  await page.getByRole('navigation').getByRole('button',{name:'Конфигурация',exact:true}).click();
  await expect(page.locator('textarea')).toHaveCount(0);
  await page.getByLabel('Уровень журналирования',{exact:true}).selectOption('debug');
  await page.getByLabel('Минимальная доля нового пула',{exact:true}).fill('0.5');
  const sent = page.waitForRequest(request => request.url().endsWith('/api/operations') && request.method() === 'POST');
  await page.route('**/api/operations', async route => { if(route.request().method()==='POST') await route.fulfill({status:409,json:{ok:false,error:{code:'revision_mismatch'}}}); else await route.fallback(); });
  await page.getByRole('button',{name:'Сохранить и применить с завершением сессий'}).click();
  const body = (await sent).postDataJSON();
  expect(body.body).toEqual({general:{log_level:'debug',me_pool_min_fresh_ratio:0.5}}); expect(body.revisions[first]).toBe('config-before');
  await expect(page.getByRole('alert')).toContainText('revision_mismatch');
  await expect(page.getByLabel('Уровень журналирования',{exact:true})).toHaveValue('debug');
  await expect(page.getByLabel('Минимальная доля нового пула',{exact:true})).toHaveValue('0.5');
  await page.getByLabel('Минимальная доля нового пула',{exact:true}).fill('');
  expect(await page.getByLabel('Минимальная доля нового пула',{exact:true}).evaluate((el: HTMLInputElement) => el.validity.valid)).toBe(false);
});

test('shaping distinguishes shared-IP users, real waits and unavailable lifetime telemetry', async ({ page }) => {
  await fixture(page,'viewer');
  await page.route('**/api/nodes/*/read*', async route => {
    const target = new URL(route.request().url()).searchParams.get('path') ?? '';
    const user = (username: string, waiting: number) => ({username,flows:2,speed_bps:[1000000,5000000],window_bytes:[1048576,2097152],waiting_flows:[0,waiting],bulk_flows:[0,1],wait_ms:[0,waiting*250]});
    const value = target.startsWith('/v1/runtime/shaping') ? {enabled:true,policy:{up_bps:180000000,down_bps:180000000,priority_percent:25,max_ips:32768},active_users:2,waiting_users:1,active_flows:4,retained_ips:1,users:[user('alice',1),user('bob',0)],ips:[{ip:'192.0.2.100',flows:4,speed_bps:[1000000,10000000],epoch_bytes:[[100,200],[300,400]],waiting_flows:[0,1]}]} : target === '/v1/fleet/traffic' ? {active_users:2,current_connections:4,telemetry_user_enabled:false} : null;
    if (value) await route.fulfill({json:{ok:true,data:{status:200,response:{ok:true,data:value,revision:'v1'}}}}); else await route.fallback();
  });
  await page.getByRole('navigation').getByRole('button',{name:'Шейпер',exact:true}).click();
  await expect(page.getByText('Трафик пользователей',{exact:true})).toBeVisible();
  const alice = page.getByRole('row').filter({hasText:'alice'}), bob = page.getByRole('row').filter({hasText:'bob'});
  await expect(alice).toContainText('Ожидает бюджета'); await expect(bob).not.toContainText('Ожидает бюджета');
  await expect(alice).toContainText('250'); await expect(alice).toContainText('Телеметрия отключена');
  await expect(page.getByRole('button',{name:'Применить политику шейпера'})).toBeDisabled();
  await page.screenshot({path:'/private/tmp/telemt-ru-shaping.png',fullPage:true});
});

test('diagnostics renders named facts and ignores responses from an abandoned tab', async ({page}) => {
  await fixture(page,'viewer');
  await page.route('**/api/nodes/*/read*', async route => {
    const target = new URL(route.request().url()).searchParams.get('path');
    if (target === '/v1/runtime/me_pool_state') { await new Promise(resolve => setTimeout(resolve,400)); await route.fulfill({json:{ok:true,data:{status:200,response:{ok:true,data:{pending_writer_deficit:999},revision:'old'}}}}); }
    else if (target === '/v1/runtime/nat_stun') await route.fulfill({json:{ok:true,data:{status:200,response:{ok:true,data:{nat_probe_enabled:true,stun:{live_total:3}},revision:'new'}}}});
    else await route.fallback();
  });
  await page.getByRole('navigation').getByRole('button',{name:'Диагностика',exact:true}).click();
  await page.getByRole('button',{name:'NAT / STUN',exact:true}).click();
  await expect(page.getByText('Проверки NAT включены',{exact:true})).toBeVisible();
  await expect(page.getByText('Действующих серверов',{exact:true})).toBeVisible();
  await page.waitForTimeout(500);
  await expect(page.getByText('Недостающие соединения',{exact:true})).toHaveCount(0);
  await expect(page.locator('pre,textarea')).toHaveCount(0);
  await page.screenshot({path:'/private/tmp/telemt-ru-diagnostics.png',fullPage:true});
});

test('overview uses selected node history and renders Russian charts on mobile', async ({page}) => {
  await page.setViewportSize({width:390,height:844}); await fixture(page,'owner');
  await page.route('**/api/nodes/*/history', async route => route.fulfill({json:{ok:true,data:Array.from({length:12},(_,index)=>({at:new Date(Date.now()-(11-index)*60000).toISOString(),snapshot:{upload_bps:index===4?null:1000000*(index+1),download_bps:5000000*(index+1),traffic:{active_users:index+2,current_connections:(index+2)*5}}}))}}));
  await page.getByLabel('Просматриваемый сервер',{exact:true}).selectOption(second);
  await expect(page.getByRole('img',{name:/Скорость трафика/})).toBeVisible();
  await expect(page.getByRole('img',{name:/Пользователи и соединения/})).toBeVisible();
  expect(await page.evaluate(()=>document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await page.screenshot({path:'/private/tmp/telemt-ru-charts-mobile.png',fullPage:true});
});

test('explicit shaping reload resets edits even when the revision is unchanged', async ({page}) => {
  await fixture(page,'operator');
  await page.getByRole('navigation').getByRole('button',{name:'Шейпер',exact:true}).click();
  const field = page.getByLabel('Доступный канал к клиентам (бит/с)',{exact:true});
  await field.fill('123456');
  page.once('dialog',dialog=>dialog.accept());
  await page.getByRole('button',{name:'Загрузить сохранённую политику',exact:true}).click();
  await expect(field).toHaveValue('100000000');
});

test('DNS creation uses described fields and preserves selected members and record families', async ({page}) => {
  await fixture(page,'owner');
  await page.getByRole('navigation').getByRole('button',{name:'Переключение DNS',exact:true}).click();
  await page.getByRole('button',{name:'Создать группу',exact:true}).click();
  const form = page.getByRole('dialog');
  await expect(form.locator('textarea')).toHaveCount(0);
  await form.getByLabel('Название группы',{exact:true}).fill('Проверка');
  await form.getByLabel('Домен прокси',{exact:true}).fill('proxy.example.org');
  await form.getByLabel('Идентификатор зоны Cloudflare',{exact:true}).fill('zone-id');
  await form.getByLabel('Идентификатор записи A',{exact:true}).fill('a-id');
  await form.getByLabel('Токен Cloudflare',{exact:true}).fill('fixture-token');
  await form.getByLabel('Активный сервер',{exact:true}).selectOption(second);
  const sent=page.waitForRequest(request=>request.url().endsWith('/api/groups') && request.method()==='POST');
  await form.getByRole('button',{name:'Сохранить',exact:true}).click();
  expect((await sent).postDataJSON()).toEqual({name:'Проверка',domain:'proxy.example.org',zone_id:'zone-id',record_a_id:'a-id',record_aaaa_id:'',token:'fixture-token',active_node:second,node_ids:[first,second]});
});

test('redesign desktop themes and sidebar preferences persist', async ({ page }) => {
  await page.emulateMedia({ colorScheme: 'light' });
  await fixture(page, 'owner');
  await page.route('**/api/nodes/*/connections-map', route => route.fulfill({ json: { ok: true, data: { available: true, generated_at: new Date().toISOString(), database: { status: 'ready', build_epoch: 1790812800 }, connections: 24, active_users: 9, total_ips: 3, cleanup_pending: 0, truncated: false, located_ips: 3, non_public_ips: 0, ips: [{ ip: '192.0.2.10', connections: 12, users: 5, location: { latitude: 55.75, longitude: 37.62, city: 'Москва', country: 'Россия' } }, { ip: '192.0.2.11', connections: 8, users: 3, location: { latitude: 59.94, longitude: 30.31, city: 'Санкт-Петербург', country: 'Россия' } }, { ip: '192.0.2.12', connections: 4, users: 1, location: { latitude: 52.52, longitude: 13.4, city: 'Берлин', country: 'Германия' } }], endpoints: [{ ip: '192.0.2.1', location: { latitude: 60.17, longitude: 24.94, city: 'Хельсинки', country: 'Финляндия' } }] } } }));
  await page.route('**/api/nodes/*/history', route => route.fulfill({ json: { ok: true, data: Array.from({ length: 20 }, (_, i) => ({ at: new Date(Date.now() - (20 - i) * 60000).toISOString(), snapshot: { upload_bps: 1000000 + (i % 5) * 500000, download_bps: 5000000 + (i % 7) * 1000000, traffic: { active_users: 9 + i % 4, current_connections: 24 + i % 8 } } })) } }));
  await page.reload();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');
  await expect(page.locator('[data-map-point]')).toHaveCount(3);
  await expect(page.getByRole('button', { name: 'Развернуть меню' })).toBeVisible();
  await page.getByRole('button', { name: 'Развернуть меню' }).click();
  await expect(page.locator('.sidebar .brand-name')).toBeVisible();
  await page.screenshot({ path: '/private/tmp/telemt-redesign-light.png', fullPage: true });
  await page.getByRole('combobox', { name: 'Тема оформления' }).selectOption('dark');
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  await page.reload();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  await expect(page.getByRole('button', { name: 'Свернуть меню' })).toBeVisible();
  await expect(page.locator('[data-map-point]')).toHaveCount(3);
  await page.screenshot({ path: '/private/tmp/telemt-redesign-dark.png', fullPage: true });
  await page.getByRole('navigation').getByRole('button', { name: 'Шейпер', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Политика шейпера' })).toBeVisible();
  await page.screenshot({ path: '/private/tmp/telemt-redesign-shaping.png', fullPage: true });
  await page.getByRole('combobox', { name: 'Тема оформления' }).selectOption('system');
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');
});

test('redesign mobile dock reaches secondary pages and keeps dialogs usable', async ({ page }) => {
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  await page.setViewportSize({ width: 390, height: 844 });
  await fixture(page, 'owner');
  await page.getByRole('button', { name: 'Ещё разделы' }).click();
  await expect(page.getByRole('dialog', { name: 'Разделы Fleet' })).toBeVisible();
  await page.getByRole('dialog').getByRole('button', { name: 'Диагностика', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Диагностика', exact: true })).toBeVisible();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await page.screenshot({ path: '/private/tmp/telemt-redesign-mobile.png', fullPage: true });
  await page.getByRole('navigation').getByRole('button', { name: 'Серверы', exact: true }).click();
  await page.getByRole('button', { name: 'Добавить сервер', exact: true }).click();
  await expect(page.getByRole('dialog')).toBeVisible();
  await expect(page.getByRole('dialog').getByRole('button', { name: 'Сохранить', exact: true })).toBeEnabled();
  await page.screenshot({ path: '/private/tmp/telemt-redesign-mobile-form.png', fullPage: true });
  await page.getByRole('button', { name: 'Закрыть окно' }).click();
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  expect(errors).toEqual([]);
});

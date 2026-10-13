import { useEffect, useState } from 'react';
import { api, type Json, object, list, readNode } from './api';
import { DataView } from './data-view';
import { Table, Badge } from './components';
const diagnostics = [
  { name:'Пул Middle Proxy', path:'/v1/runtime/me_pool_state', help:'Поколения пула, готовность соединений записи и пополнение ресурсов.' },
  { name:'Качество Middle Proxy', path:'/v1/runtime/me_quality', help:'Ошибки, задержки и качество маршрутов Middle Proxy.' },
  { name:'Дата-центры', path:'/v1/stats/dcs', help:'Соединения и измеренная задержка до дата-центров Telegram.' },
  { name:'Исходящие маршруты', path:'/v1/runtime/upstream_quality', help:'Доступность маршрутов к Telegram независимо от доступности API ноды.' },
  { name:'NAT / STUN', path:'/v1/runtime/nat_stun', help:'Обнаруженные внешние адреса, NAT и результаты отражения STUN.' },
  { name:'Самопроверка KDF', path:'/v1/runtime/me-selftest', help:'Проверка параметров криптографической маршрутизации без изменения ключей.' },
  { name:'Приём подключений', path:'/v1/runtime/gates', help:'Ограничения приёма новых соединений и состояние завершения сессий.' },
  { name:'Состояние WEB', path:'/v1/runtime/web/status', help:'Доступность WEB, приём запросов, ёмкость процесса и обычный сайт.' },
  { name:'Сессии WEB', path:'/v1/runtime/web/sessions', help:'Активные сессии, клиенты и потоки выбранного сервера.' },
  { name:'События', path:'/v1/runtime/events/recent', help:'Последние диагностические события ноды; история ограничена размером буфера.' },
];
export function Diagnostics({ nodeId, canWrite, run }: { nodeId: string; canWrite: boolean; run: (work: () => Promise<void>) => void }) {
  const [tab, setTab] = useState(diagnostics[0]), [data, setData] = useState<Json>(null), [error, setError] = useState(''), [operation, setOperation] = useState(''), [at, setAt] = useState<number | null>(null), [refresh, setRefresh] = useState(0);
  useEffect(() => { let active = true; let fetching = false; setData(null); setAt(null); setError(''); const fetch = async () => { if (fetching) return; fetching = true; try { const response = await readNode(nodeId, tab.path); if (active) { setData(response.data); setError(''); setAt(Date.now()); } } catch (error) { if (active) setError(String(error)); } finally { fetching = false; } }; void fetch(); const timer = setInterval(fetch, 5000); return () => { active = false; clearInterval(timer); }; }, [nodeId, tab.path, refresh]);
  const command = (path: string, extra: Record<string, Json> = {}) => run(async () => {
    const status = await readNode(nodeId, '/v1/runtime/web/status'); const instance = object(object(status.data).runtime).runtime_instance;
    if (typeof instance !== 'string' || !/^[0-9a-f]{32}$/.test(instance)) throw new Error('Активный процесс WEB недоступен');
    const result = await api<{ id: string }>('operations', 'POST', { node_ids: [nodeId], method: 'POST', path, body: { runtime_instance: instance, ...extra }, revisions: { [nodeId]: status.revision } }); setOperation(result.id);
  });
  const web = tab.path.includes('/web/'); const sessions = tab.path.endsWith('/sessions');
  return <section><div className="section-title"><h2>Диагностика ноды</h2><button className="quiet" onClick={() => setRefresh(refresh + 1)}>Обновить диагностику</button></div><div className="tabs">{diagnostics.map(item => <button className={tab.path === item.path ? 'active' : 'quiet'} aria-pressed={tab.path === item.path} key={item.path} onClick={() => setTab(item)}>{item.name}</button>)}</div><div className="diagnostic-intro"><h3>{tab.name}</h3><p>{tab.help}</p><Badge value={error ? 'stale' : at ? 'reachable' : 'unknown'} /><span className="muted"> {at ? `Измерение ${new Date(at).toLocaleTimeString('ru-RU')}` : 'Ожидание измерений'}</span></div>{error && <p role="alert" className="error">{error}. Ранее полученные данные могут быть устаревшими.</p>}<DataView value={sessions ? Object.fromEntries(Object.entries(object(data)).filter(([key]) => key !== 'sessions')) : data} />
    {sessions && <Table rows={list(object(data).sessions)} columns={Array.from(new Set(['session_ref','client_ip','user','host',...list(object(data).sessions).flatMap(row => Object.keys(object(row)))]))} actions={canWrite ? row => <button className="danger" onClick={() => { if (confirm('Закрыть эту WEB-сессию?')) command('/v1/runtime/web/sessions/close', { selector: { kind: 'refs', session_refs: [row.session_ref] } }); }}>Закрыть сессию</button> : undefined} />}
    {web && canWrite && <div className="runtime-actions"><p>Команды действуют только на просматриваемый сервер и проверяют идентификатор текущего процесса.</p><div className="actions"><button onClick={() => command('/v1/runtime/web/lifecycle/pause')}>Приостановить новые WEB-сессии</button><button onClick={() => { if (confirm('Завершить WEB-сессии на этом сервере в течение 300 секунд?')) command('/v1/runtime/web/lifecycle/drain', { timeout_secs: 300 }); }}>Завершить WEB-сессии</button><button onClick={() => command('/v1/runtime/web/lifecycle/resume')}>Возобновить WEB-сессии</button><button className="quiet" onClick={() => command('/v1/runtime/web/debug/clear')}>Очистить историю отладки</button><button className="quiet" onClick={() => command('/v1/runtime/web/carrier-learning/reset')}>Сбросить обучение транспорта</button></div></div>}{operation && <p>Операция {operation} поставлена в очередь. Завершение отслеживается в разделе «Операции».</p>}</section>;
}

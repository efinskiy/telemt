import { lazy, Suspense, useEffect, useState } from 'react';
import { api, type Json, type Node, object } from './api';
import { Chart, type Sample } from './charts';
import { FleetSummary } from './fleet-summary';
import { OverviewServers } from './overview-servers';
const ConnectionsMap = lazy(() => import('./connections-map').then(module => ({ default: module.ConnectionsMap })));
const numeric = (value: Json | undefined, divisor = 1) => typeof value === 'number' && Number.isFinite(value) ? value / divisor : null;
export function Overview({ nodes, nodeId, inspect }: { nodes: Node[]; nodeId: string; inspect: (id: string) => void }) {
  const [history, setHistory] = useState<Json[]>([]), [error, setError] = useState(''), [hours, setHours] = useState(1);
  useEffect(() => { let active = true; setHistory([]); setError(''); if (!nodeId) return; const fetch = async () => { try { const rows = await api<Json[]>(`nodes/${nodeId}/history`); if (active) { setHistory(rows); setError(''); } } catch (error) { if (active) setError(String(error)); } }; void fetch(); const timer = setInterval(fetch, 30000); return () => { active = false; clearInterval(timer); }; }, [nodeId]);
  const [now, setNow] = useState(Date.now());
  useEffect(() => { const timer = setInterval(() => setNow(Date.now()), 5000); return () => clearInterval(timer); }, []);
  const node = nodes.find(row => row.id === nodeId);
  const samples: Sample[] = history.map(row => { const item = object(row), snapshot = object(item.snapshot), traffic = object(snapshot.traffic); return { at: Date.parse(String(item.minute ?? item.at)), values: [numeric(snapshot.upload_bps, 1e6), numeric(snapshot.download_bps, 1e6), traffic.active_ips_incomplete === true ? null : numeric(traffic.active_ips), numeric(traffic.current_connections)] }; }).filter(row => row.at >= Date.now() - hours * 3600000);
  return <><FleetSummary nodes={nodes} now={now} />
    <div className="section-title"><div><h2>Динамика · {node?.name ?? 'выберите сервер'}</h2><p className="muted">Минутная история: скорость усредняется, количество подключений — последнее измерение минуты.</p></div><label className="label">Период<select aria-label="Период графиков" value={hours} onChange={event => setHours(Number(event.target.value))}><option value={1}>Последний час</option><option value={6}>6 часов</option><option value={24}>24 часа</option></select></label></div>{error && <p role="alert" className="error">История недоступна: {error}</p>}
    <div className="chart-grid"><Chart title="Скорость трафика" samples={samples} series={['От клиента', 'К клиенту']} unit="Мбит/с" /><Chart title="IP-клиенты и соединения" samples={samples.map(row => ({ at: row.at, values: row.values.slice(2) }))} series={['IP-клиенты', 'Соединения']} unit="шт." /></div>
    <div id="overview-map" />
    <Suspense fallback={<p className="muted">Загрузка карты…</p>}><ConnectionsMap key={nodeId} node={node} /></Suspense>
    <OverviewServers nodes={nodes} inspect={id => { inspect(id); document.getElementById('overview-map')?.scrollIntoView({ behavior: 'smooth', block: 'start' }); }} />
    </>;
}

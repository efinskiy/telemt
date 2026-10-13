import { useEffect, useState } from 'react';
import { api, type Json, object, list, readNode } from './api';
import { parseMap, place, type MapSnapshot } from './connection-map-data';
import { capacity, count, sum, value, volume } from './overview-model';

/** Only the expanded card polls IP details; cancellation stops requests when it closes. */
export function OverviewClients({ nodeId }: { nodeId: string }) {
  const [data, setData] = useState<MapSnapshot | null>(null), [shaping, setShaping] = useState<Record<string, Json>>({}), [error, setError] = useState(''), [notice, setNotice] = useState(''), [page, setPage] = useState(0), [search, setSearch] = useState(''), [refresh, setRefresh] = useState(0);
  useEffect(() => {
    const controller = new AbortController(); let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        const [map, shape] = await Promise.allSettled([
          api<unknown>(`nodes/${nodeId}/connections-map`, 'GET', undefined, controller.signal).then(parseMap),
          readNode(nodeId, '/v1/runtime/shaping?offset=0&limit=1000', controller.signal),
        ]);
        if (controller.signal.aborted) return;
        if (map.status === 'rejected') throw map.reason;
        if (!map.value.available) { setData(null); setNotice('Список подключений недоступен. Проверьте доступность и версию ноды.'); }
        else { setData(map.value); setNotice(''); }
        setShaping(shape.status === 'fulfilled' ? object(shape.value.data) : {}); setError('');
      } catch (error) { if (!controller.signal.aborted) setError(String(error)); }
      finally { if (!controller.signal.aborted) timer = setTimeout(poll, 10000); }
    };
    void poll(); return () => { controller.abort(); clearTimeout(timer); };
  }, [nodeId, refresh]);
  const metrics = new Map(list(shaping.ips).map(row => [String(object(row).ip), object(row)]));
  const speed = (ip: string) => shaping.enabled === true ? sum(list(metrics.get(ip)?.speed_bps)[0], list(metrics.get(ip)?.speed_bps)[1]) : null;
  const rows = [...(data?.ips ?? [])].filter(row => `${row.ip} ${row.location ? place(row.location) : ''}`.toLocaleLowerCase('ru').includes(search.toLocaleLowerCase('ru'))).sort((a, b) => (speed(b.ip) ?? -1) - (speed(a.ip) ?? -1) || b.connections - a.connections || a.ip.localeCompare(b.ip));
  const last = Math.max(0, Math.ceil(rows.length / 25) - 1), current = Math.min(page, last);
  const stale = !!data && (!!error || Date.now() - Date.parse(data.generated_at) > 30000);
  return <div className="server-clients"><div className="section-title"><div><h4>Активные IP · {count(data?.total_ips)}</h4><small className={stale ? 'stale' : 'muted'}>{stale ? 'Последний снимок устарел' : data ? `Снимок ${new Date(data.generated_at).toLocaleTimeString('ru-RU')}` : 'Ожидание данных'}</small></div><button className="quiet" onClick={() => setRefresh(v => v + 1)}>Обновить IP</button></div>
    {error && <p role="alert" className="error">IP не обновлены: {error}</p>}{notice && <p className="notice">{notice}</p>}
    <label className="label">Поиск IP и города<input value={search} onChange={e => { setSearch(e.target.value); setPage(0); }} /></label>
    <p className="muted">Скорость — сумма отправки и получения. Измерения шейпера доступны для первых 1000 IP по адресу; сортировка по скорости действует только для полученных измерений. {shaping.enabled !== true ? 'Шейпер выключен или недоступен — скорость IP неизвестна.' : ''}</p>
    {data?.truncated && <p className="notice">Неполный снимок: {count(data.ips.length)} из {count(data.total_ips)} IP.</p>}
    {data && data.database.status !== 'ready' && <p className="notice">Локальная база геолокации недоступна.</p>}
    {data && data.cleanup_pending > 0 && <p className="notice">Ожидается учёт {count(data.cleanup_pending)} отключений; числа могут включать закрытые соединения.</p>}
    <div className="table-wrap"><table><thead><tr><th>IP / расположение</th><th>Скорость ↑ + ↓</th><th>За минуту</th><th>Соединения / аккаунты</th></tr></thead><tbody>{rows.slice(current * 25, current * 25 + 25).map(row => { const metricsRow = metrics.get(row.ip), window = shaping.enabled === true ? list(metricsRow?.window_bytes) : []; return <tr key={row.ip}><td><code>{row.ip}</code><small>{row.location ? place(row.location) : 'Расположение неизвестно'}</small></td><td>{capacity(speed(row.ip))}</td><td>{volume(sum(window[0], window[1]))}</td><td>{count(row.connections)} / {count(row.users)}</td></tr>; })}</tbody></table></div>
    {data && !rows.length && <p className="empty">Активных IP по этому фильтру нет.</p>}
    <div className="actions"><button className="quiet" disabled={current === 0} onClick={() => setPage(current - 1)}>Назад</button><span>{current + 1} / {last + 1}</span><button className="quiet" disabled={current === last} onClick={() => setPage(current + 1)}>Далее</button><small className="muted">{count(rows.length)} IP</small></div>
    {value(shaping.retained_ips) !== null && Number(shaping.retained_ips) > 1000 && <small className="muted">В шейпере больше 1000 записей; отсутствующие измерения не считаются нулевым трафиком.</small>}
  </div>;
}

import { useEffect, useMemo, useState } from 'react';
import { api, type Node } from './api';
import { Metric } from './charts';
import { number, date } from './labels';
import { ConnectionGlobe } from './connection-globe';
import { MAX_MAP_POINTS, clusterClients, parseMap, place, type MapSnapshot } from './connection-map-data';
import './connections-map.css';

const reasons: Record<string, string> = { node_disabled: 'Нода отключена от мониторинга.', node_update_required: 'Обновите telemt на ноде: требуется API активных подключений.' };
export function ConnectionsMap({ node }: { node: Node | undefined }) {
  const [snapshot, setSnapshot] = useState<MapSnapshot | null>(null), [error, setError] = useState(''), [notice, setNotice] = useState(''), [loading, setLoading] = useState(true), [received, setReceived] = useState(0);
  const [selected, setSelected] = useState<string | null>(null), [search, setSearch] = useState(''), [page, setPage] = useState(0), [refresh, setRefresh] = useState(0);
  useEffect(() => {
    const controller = new AbortController(); let timer: ReturnType<typeof setTimeout> | undefined;
    setSnapshot(null); setReceived(0); setError(''); setNotice(''); setSelected(null); setPage(0); setLoading(true);
    if (!node?.enabled) { setLoading(false); setNotice(node ? reasons.node_disabled : 'Выберите сервер для просмотра подключений.'); return; }
    const poll = async () => {
      try {
        const response = parseMap(await api<unknown>(`nodes/${node.id}/connections-map`, 'GET', undefined, controller.signal));
        if (controller.signal.aborted) return;
        setError('');
        if (response.available) { setSnapshot(response); setReceived(Date.now()); setNotice(''); }
        else { setSnapshot(null); setNotice(reasons[response.reason] ?? 'Данные карты пока недоступны.'); }
      } catch (e) { if (!controller.signal.aborted) setError(String(e)); }
      finally { if (!controller.signal.aborted) { setLoading(false); timer = setTimeout(poll, 5000); } }
    };
    void poll(); return () => { controller.abort(); clearTimeout(timer); };
  }, [node?.id, node?.enabled, refresh]);
  const clusters = useMemo(() => clusterClients(snapshot?.ips ?? []), [snapshot]);
  const current = clusters.find(row => row.id === selected);
  const clients = useMemo(() => [...(current?.ips ?? snapshot?.ips ?? [])].filter(row => `${row.ip} ${row.location ? place(row.location) : ''}`.toLocaleLowerCase('ru').includes(search.toLocaleLowerCase('ru'))).sort((a, b) => b.connections - a.connections || a.ip.localeCompare(b.ip)), [current, snapshot, search]);
  const lastPage = Math.max(0, Math.ceil(clients.length / 25) - 1), actualPage = Math.min(page, lastPage);
  const stale = !!snapshot && (!!error || Date.now() - received > 15000 || Date.now() - Date.parse(snapshot.generated_at) > 30000);
  const choose = (id: string | null) => { setSelected(id); setPage(0); };
  return <section className="connections-map" aria-label="Карта подключений">
    <div className="section-title"><div><h2>Подключения на карте · {node?.name ?? 'сервер не выбран'}</h2><p className="muted">Активные клиенты → нода. Обновление каждые 5 секунд. Координаты приблизительные.</p></div><button disabled={loading || !node?.enabled} onClick={() => setRefresh(v => v + 1)}>Обновить карту</button></div>
    <div className="map-status" role="status"><span className={stale ? 'map-stale' : 'map-live'}>{loading ? 'Загрузка подключений…' : snapshot ? stale ? 'Данные устарели' : 'Текущие подключения' : 'Нет измерения'}</span>{snapshot && <span>Снимок: {date(snapshot.generated_at)}</span>}</div>
    {error && <p role="alert" className="error">Карта не обновлена: {error}{snapshot ? ' Отображён последний полученный снимок.' : ''}</p>}{notice && <p className="notice">{notice}</p>}
    {snapshot && <><div className="cards map-metrics"><Metric label="Пользователи онлайн" value={number(snapshot.active_users)} hint="Учётные записи на выбранной ноде" /><Metric label="Активные соединения" value={number(snapshot.connections)} hint="Включая логические WEB-потоки" /><Metric label="IP на карте" value={`${number(snapshot.located_ips)} / ${number(snapshot.total_ips)}`} hint="Адреса с известными координатами" /><Metric label="Без координат" value={number(snapshot.ips.length - snapshot.located_ips)} hint={`Из них непубличных: ${number(snapshot.non_public_ips)}`} /></div>
      {snapshot.database.status !== 'ready' && <p className="notice">{snapshot.database.status === 'missing' ? 'База геолокации не установлена.' : 'База геолокации недоступна или повреждена.'} Установите локальную базу DB-IP City Lite по инструкции развёртывания Fleet и перезапустите кабинет. Счётчики и список IP доступны.</p>}
      {snapshot.truncated && <p className="notice">Неполный снимок: показано {number(snapshot.ips.length)} IP из {number(snapshot.total_ips)} обнаруженных. Нода ограничивает ответ 10 000 адресами; карта не отражает все подключения.</p>}
      {snapshot.cleanup_pending > 0 && <p className="notice">Ожидается обработка {number(snapshot.cleanup_pending)} событий отключения. Счётчики могут кратковременно включать закрытые подключения.</p>}
    </>}
    <div className="map-layout"><ConnectionGlobe snapshot={snapshot} clusters={clusters} selected={selected} select={choose} /><div className="map-regions" role="complementary" aria-label="Регионы подключений"><h3>География подключений</h3><p className="muted">По числу соединений; близкие точки объединены.</p>{clusters.length ? clusters.slice(0, MAX_MAP_POINTS).map(row => <button key={row.id} aria-pressed={selected === row.id} onClick={() => choose(selected === row.id ? null : row.id)}><span>{place(row.location)}<small>{row.ips.length} IP · IPv{row.family}</small></span><strong>{number(row.connections)}</strong></button>) : <p className="muted">{snapshot?.connections === 0 ? 'Сейчас нет активных подключений.' : 'Нет подключений с известными координатами.'}</p>}</div></div>
    {clusters.length > MAX_MAP_POINTS && <p className="notice">На карте показаны {MAX_MAP_POINTS} самых активных групп из {clusters.length}. Все полученные IP доступны в таблице.</p>}
    {snapshot && !snapshot.endpoints.some(row => row.location) && <p className="muted">Координаты публичного IP ноды неизвестны — линии к ноде не построены.</p>}
    {snapshot && <><div className="section-title"><h3>{current ? `Подключения · ${place(current.location)}` : 'Активные IP клиентов'}</h3>{current && <button onClick={() => choose(null)}>Все подключения</button>}<label>Поиск по IP или городу<input aria-label="Поиск подключений" value={search} onChange={event => { setSearch(event.target.value); setPage(0); }} placeholder="IP, город, страна" /></label></div>
      <div className="table-wrap"><table><thead><tr><th>IP клиента</th><th>Расположение</th><th>Соединения</th><th>Пользователи на IP</th></tr></thead><tbody>{clients.slice(actualPage * 25, actualPage * 25 + 25).map(row => <tr key={row.ip}><td><code>{row.ip}</code></td><td>{row.location ? place(row.location) : 'Координаты неизвестны'}</td><td>{number(row.connections)}</td><td>{number(row.users)}</td></tr>)}</tbody></table>{!clients.length && <p className="muted">Подключений по этому фильтру нет.</p>}</div>
      <div className="map-pagination"><button disabled={actualPage === 0} onClick={() => setPage(actualPage - 1)}>Назад</button><span>{actualPage + 1} / {lastPage + 1} · {number(clients.length)} IP</span><button disabled={actualPage === lastPage} onClick={() => setPage(actualPage + 1)}>Далее</button></div><p className="muted">Пользователи на разных IP могут повторяться. При VPN или NAT точка соответствует выходному адресу сети. Публичные IP ноды берутся из её настроек в Fleet.</p></>}
    <div className="map-attribution"><span>Геолокация: <a href="https://db-ip.com" target="_blank" rel="noreferrer">DB-IP</a> · карта: <a href="https://www.naturalearthdata.com" target="_blank" rel="noreferrer">Natural Earth</a></span><span>{snapshot?.database.build_epoch ? `База от ${date(new Date(snapshot.database.build_epoch * 1000).toISOString())} · ` : ''}IP обрабатываются локально на Fleet</span></div>
  </section>;
}

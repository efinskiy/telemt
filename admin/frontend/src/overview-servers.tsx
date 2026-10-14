import { useEffect, useState, type ReactNode } from 'react';
import { type Node, type Json, object, list, text } from './api';
import { alerts, bulkLimit, capacity, count, dcRows, dcFresh, duration, fresh, percent, ratio, sum, used, value, volume } from './overview-model';
import { OverviewClients } from './overview-clients';
import { date } from './labels';
import { ipCount } from './fleet-summary';
import './overview-servers.css';

function Fact({ label, children }: { label: string; children: ReactNode }) { return <div className="server-fact"><dt>{label}</dt><dd>{children}</dd></div>; }
function Signal({ name, state }: { name: string; state: boolean | null }) { return <span className={`server-signal ${state === null ? 'unknown' : state ? 'good' : 'bad'}`} aria-label={`${name}: ${state === null ? 'нет актуальных данных' : state ? 'доступен' : 'недоступен'}`}><i />{name}</span>; }
function Resource({ label, current, total }: { label: string; current: Json | undefined; total: Json | undefined }) {
  const fill = ratio(current, total); return <div className="server-resource"><div><span>{label}</span><strong>{volume(current)} / {volume(total)}</strong></div><div className="server-gauge" aria-label={`${label}: ${percent(fill)}`}><span className={fill !== null && fill >= 90 ? 'warning' : ''} style={{ width: `${Math.min(100, fill ?? 0)}%` }} /></div></div>;
}
function ServerCard({ node, now, expanded, expand, inspect }: { node: Node; now: number; expanded: boolean; expand: () => void; inspect: () => void }) {
  const snapshot = node.snapshot, traffic = object(snapshot.traffic), info = object(snapshot.info), summary = object(snapshot.summary), host = object(snapshot.host), disk = object(host.disk), shaping = object(snapshot.shaping), policy = object(shaping.policy), gates = object(snapshot.gates);
  const recent = fresh(node, now), dcs = dcRows(node), dc = object(snapshot.dcs), dcRecent = dcFresh(node, now);
  const available = recent && snapshot.management === 'reachable', age = node.collected_at ? (now - Date.parse(node.collected_at)) / 1000 : null;
  const bad = ratio(summary.connections_bad_total, summary.connections_total);
  const totalWriters = (key: string) => dcs.length && dcs.every(row => value(row[key]) !== null) ? dcs.reduce((sum, row) => sum + Number(row[key]), 0) : null;
  const writers = totalWriters('alive_writers'), required = totalWriters('required_writers');
  const addresses = [node.document.ipv4, node.document.ipv6].filter(v => typeof v === 'string' && v).join(' · ');
  return <article className={`server-summary${recent ? '' : ' server-summary-stale'}`} aria-label={`Сводка сервера ${node.name}`}>
    <header className="server-summary-header"><div><h3>{node.name}</h3><code>{addresses || 'Публичный IP не указан'}</code></div><div className="server-signals"><Signal name="API" state={recent && typeof snapshot.management === 'string' ? snapshot.management === 'reachable' : null} /><Signal name="Приём" state={available && typeof snapshot.ready === 'boolean' ? snapshot.ready : null} /><Signal name="ME" state={dcRecent && dc.middle_proxy_enabled === true && dcs.length && dcs.every(row => value(row.available_pct) !== null) ? dcs.every(row => row.available_pct === 100) : null} /></div></header>
    <div className="server-snapshot"><span className={!recent ? 'stale' : 'muted'}>{!node.enabled ? 'Мониторинг отключён' : !recent ? 'Данные устарели' : 'Свежее измерение'} · {duration(age)} назад</span><button className="quiet" onClick={inspect}>Карта и графики</button></div>
    <div className="server-block"><h4>Трафик и подключения <small>Направления относительно клиента</small></h4><dl className="server-facts">
      <Fact label="IP-клиенты онлайн">{available ? ipCount(node) : 'Нет данных'}</Fact><Fact label="Аккаунты онлайн">{count(traffic.active_users)}</Fact><Fact label="Соединения">{count(traffic.current_connections)}</Fact>
      <Fact label="Трафик за период счётчика">{traffic.available === true ? volume(sum(traffic.octets_from_client, traffic.octets_to_client)) : 'Нет данных'}</Fact><Fact label="Трафик Upload ↑">{traffic.available === true ? volume(traffic.octets_from_client) : 'Нет данных'}</Fact><Fact label="Трафик Download ↓">{traffic.available === true ? volume(traffic.octets_to_client) : 'Нет данных'}</Fact><Fact label="Скорость ↑ + ↓">{capacity(sum(snapshot.upload_bps, snapshot.download_bps))}</Fact>
      <Fact label="От клиента ↑">{capacity(snapshot.upload_bps)}</Fact><Fact label="К клиенту ↓">{capacity(snapshot.download_bps)}</Fact>
      <Fact label="Канал шейпера ↑">{capacity(policy.up_bps)}</Fact><Fact label="Канал шейпера ↓">{capacity(policy.down_bps)}</Fact>
    </dl><small className="muted">Аккаунт может обслуживать нескольких людей. Ёмкость канала задана в политике, а не измерена тестом скорости.</small>
    <div className="server-shaping"><div className="server-line"><strong>Шейпер</strong><span>{shaping.enabled === true ? 'Включён' : shaping.enabled === false ? 'Выключен' : 'Нет данных'} · ожидают бюджета {count(shaping.waiting_users)} аккаунтов</span></div>
      <div className="table-wrap"><table><thead><tr><th>Объёмный класс</th><th>При конкуренции</th><th>Свободный канал</th></tr></thead><tbody>{(['up', 'down'] as const).map(direction => <tr key={direction}><td>{direction === 'up' ? 'От клиента ↑' : 'К клиенту ↓'}</td><td>{capacity(bulkLimit(policy, direction))}</td><td>{capacity(bulkLimit(policy, direction, true))}</td></tr>)}</tbody></table></div>
      <small className="muted">Лимиты общие для класса. Содержимое сообщений и файлов внутри зашифрованного потока не различается.</small>
    </div></div>
    <div className="server-block"><h4>Ресурсы сервера <small>{text(host.cpu_model)}</small></h4>
      {host.available !== true && <p className="notice">Метрики ОС недоступны. Проверьте версию Linux-ноды и доступ процесса к /proc.</p>}
      <div className="server-resources"><Resource label="RAM" current={used(host.memory_total_bytes, host.memory_available_bytes)} total={host.memory_total_bytes} /><Resource label="Swap" current={used(host.swap_total_bytes, host.swap_free_bytes)} total={host.swap_total_bytes} /></div>
      <dl className="server-facts"><Fact label="Uptime ОС">{duration(host.uptime_seconds)}</Fact><Fact label="Нагрузка LA1">{count(host.load1)}</Fact><Fact label="CPU занят / ядер">{percent(host.cpu_busy_percent)} / {count(object(host.cpu).cores)}</Fact><Fact label="CPU свободно">{percent(host.cpu_idle_percent)}</Fact><Fact label="SoftIRQ (весь CPU)">{percent(host.softirq_percent)}</Fact><Fact label="Файловая система /">{percent(ratio(used(disk.total_bytes, disk.available_bytes), disk.total_bytes))}</Fact><Fact label="Сокеты / TCP established">{count(host.sockets)} / {count(object(host.tcp).CurrEstab)}</Fact><Fact label="Conntrack">{count(host.conntrack_count)} / {count(host.conntrack_max)}</Fact><Fact label="Повторные TCP-сегменты">{percent(host.tcp_retransmit_percent)}</Fact><Fact label="Telemt RSS / CPU">{volume(host.process_rss_bytes)} / {count(host.process_cpu_cores)} ядра</Fact></dl>
      <small className="muted">ОС и сеть в пространстве имён процесса; в контейнере это не обязательно ресурсы всего физического сервера. Диск — ёмкость файловой системы /, не нагрузка ввода-вывода.</small>
    </div>
    <div className="server-block"><div className="server-line"><h4>Telemt {typeof info.version === 'string' ? `v${info.version}` : ''}</h4><span>Работает {duration(info.uptime_seconds)}</span></div><small className="muted">Запуск: {value(info.process_started_at_epoch_secs) !== null ? date(new Date(Number(info.process_started_at_epoch_secs) * 1000).toString()) : 'Нет данных'}</small>
      <dl className="server-facts"><Fact label="Всего подключений">{count(summary.connections_total)}</Fact><Fact label="Ошибочные">{count(summary.connections_bad_total)} · {percent(bad)}</Fact><Fact label="Таймауты handshake">{count(snapshot.handshake_timeouts_per_minute)} / мин</Fact><Fact label="Таймауты с запуска">{count(summary.handshake_timeouts_total)}</Fact></dl>
      <details className="server-errors"><summary>Причины ошибок · с запуска процесса</summary>{[['Ошибочные подключения', summary.connections_bad_by_class], ['Ошибки handshake', summary.handshake_failures_by_class]].map(([title, rows]) => <div key={String(title)}><h5>{String(title)}</h5>{list(rows).length ? <dl>{list(rows).slice(0, 32).map(row => { const item = object(row); return <Fact key={text(item.class)} label={text(item.class)}>{count(item.total)}</Fact>; })}</dl> : <p className="muted">Нет зарегистрированных причин или измерение недоступно.</p>}</div>)}</details>
      <div className="server-line"><strong>Дата-центры Telegram</strong><span className="muted">{dcRecent ? 'Актуальные данные ME' : 'Нет свежего измерения ME'}</span></div>
      {dcs.length ? <div className={`table-wrap${dcRecent ? '' : ' stale'}`}><table className="server-dcs"><thead><tr><th>DC</th><th>Эндпоинты</th><th>RTT</th><th>Сессии</th></tr></thead><tbody>{dcs.map(row => <tr key={text(row.dc)}><td><Signal name={`DC ${text(row.dc)}`} state={dcRecent && value(row.available_pct) !== null ? Number(row.available_pct) === 100 : null} /></td><td>{percent(row.available_pct)} <small>({count(row.available_endpoints)}/{Array.isArray(row.endpoints) ? count(row.endpoints.length) : '—'})</small></td><td>{count(row.rtt_ms)}{value(row.rtt_ms) !== null ? ' мс' : ''}</td><td>{count(row.load)}</td></tr>)}</tbody></table></div> : <p className="muted">{dc.middle_proxy_enabled === false ? 'Статистика ME выключена или недоступна; это не результат проверки публичного прокси.' : 'Измерение DC недоступно.'}</p>}
      <div className="server-line"><span>Соединения записи ME</span><strong>{count(writers)} / {count(required)}</strong></div><div className="server-line"><span>Маршрут</span><strong>{gates.route_mode === 'middle' ? 'Через Middle Proxy' : gates.route_mode === 'direct' ? 'Прямой' : 'Нет данных'}</strong></div>
    </div>
    <div className="server-client-toggle"><button className="quiet" aria-expanded={expanded} disabled={!node.enabled} onClick={expand}>{expanded ? 'Скрыть IP клиентов' : 'Показать IP клиентов'}</button><small className="muted">География · скорость · соединения</small></div>{expanded && <OverviewClients nodeId={node.id} />}
  </article>;
}

export function OverviewServers({ nodes, inspect }: { nodes: Node[]; inspect: (id: string) => void }) {
  const [now, setNow] = useState(Date.now()), [expanded, setExpanded] = useState<string | null>(null);
  useEffect(() => { const timer = setInterval(() => setNow(Date.now()), 5000); return () => clearInterval(timer); }, []);
  const notices = alerts(nodes, now);
  return <div className="overview-servers"><div className="section-title"><div><h2>Сводка по серверам</h2><p className="muted">Состояние каждой ноды, её трафик, ресурсы и связи с Telegram.</p></div><span className="muted">{nodes.length} серверов</span></div>
    <div className={`server-alerts${notices.length ? ' has-alerts' : ''}`} aria-label="Предупреждения серверов"><h3>Предупреждения <span>{notices.length}</span></h3>{notices.length ? notices.map((alert, i) => <button key={`${alert.node.id}-${i}`} className={`server-alert ${alert.severity}`} onClick={() => document.getElementById(`server-${alert.node.id}`)?.scrollIntoView({ behavior: 'smooth', block: 'start' })}><strong>{alert.node.name}</strong><span>{alert.message}</span></button>) : <p className="muted">По полученным измерениям предупреждений нет. Отсутствующие метрики не подтверждают исправность.</p>}</div>
    {!nodes.length && <p className="empty">Добавьте сервер для начала мониторинга.</p>}<div className="server-summary-grid">{nodes.slice(0, 20).map(node => <div id={`server-${node.id}`} key={node.id}><ServerCard node={node} now={now} expanded={expanded === node.id && node.enabled} expand={() => setExpanded(current => current === node.id ? null : node.id)} inspect={() => inspect(node.id)} /></div>)}</div>
  </div>;
}

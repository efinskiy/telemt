import { type ReactNode } from 'react';
import { type Node, object } from './api';
import { capacity, count, fresh, sum, used, value, volume } from './overview-model';
import { Icon } from './interface';
import './fleet-summary.css';

/** A node contributes only a current measurement; incomplete fleet totals carry their coverage. */
export function measured(node: Node, now: number) { return fresh(node, now) && node.snapshot.management === 'reachable'; }
export function ipCount(node: Node) {
  const traffic = object(node.snapshot.traffic);
  return `${traffic.active_ips_incomplete === true ? '≈ ' : ''}${count(traffic.active_ips)}`;
}
function Stat({ label, children, hint, icon, tone = '' }: { label: string; children: ReactNode; hint: string; icon: string; tone?: string }) {
  return <div className={`fleet-stat ${tone}`}><span className="fleet-stat-icon"><Icon name={icon} /></span><div><span>{label}</span><strong>{children}</strong><small>{hint}</small></div></div>;
}
export function FleetSummary({ nodes, now }: { nodes: Node[]; now: number }) {
  const live = nodes.filter(node => measured(node, now));
  const metric = (read: (node: Node) => number | null) => {
    const values = live.map(read).filter((v): v is number => v !== null);
    return { total: values.length ? values.reduce((a, b) => a + b, 0) : null, hint: `Свежие данные: ${values.length} из ${nodes.length} серверов` };
  };
  const ips = metric(node => value(object(node.snapshot.traffic).active_ips));
  const connections = metric(node => value(object(node.snapshot.traffic).current_connections));
  const accounts = metric(node => value(object(node.snapshot.traffic).active_users));
  const transfer = (node: Node, direction: string) => {
    const traffic = object(node.snapshot.traffic);
    return traffic.available === true && value(traffic.octets_from_client) !== null && value(traffic.octets_to_client) !== null ? value(traffic[direction]) : null;
  };
  const up = metric(node => transfer(node, 'octets_from_client')), down = metric(node => transfer(node, 'octets_to_client'));
  const upload = metric(node => value(node.snapshot.upload_bps)), download = metric(node => value(node.snapshot.download_bps));
  const ram = metric(node => { const host = object(node.snapshot.host); return host.available === true ? used(host.memory_total_bytes, host.memory_available_bytes) : null; });
  const memory = metric(node => { const host = object(node.snapshot.host); return host.available === true ? value(host.memory_total_bytes) : null; });
  const rss = metric(node => { const host = object(node.snapshot.host); return host.available === true ? value(host.process_rss_bytes) : null; });
  const incomplete = live.some(node => object(node.snapshot.traffic).active_ips_incomplete === true);
  return <div className="fleet-summary">
    <section aria-label="Подключения кластера"><div className="fleet-section-heading"><h2>Подключения</h2><span>Сейчас · обновление каждые 5 секунд</span></div><div className="fleet-stats four">
      <Stat label="IP-клиенты онлайн" icon="users" tone="green" hint={ips.hint}>{incomplete && ips.total !== null ? '≈ ' : ''}{count(ips.total)}</Stat>
      <Stat label="Активные соединения" icon="activity" hint={connections.hint}>{count(connections.total)}</Stat>
      <Stat label="Аккаунты онлайн" icon="shield" tone="violet" hint={accounts.hint}>{count(accounts.total)}</Stat>
      <Stat label="Готовые серверы" icon="server" tone="green" hint={`${nodes.length} зарегистрировано · ${nodes.filter(node => !node.enabled).length} отключено`}>{live.filter(node => node.snapshot.ready === true).length} / {nodes.length}</Stat>
    </div><p className="fleet-note">IP уникальны внутри ноды; один IP или аккаунт на нескольких нодах учитывается на каждой. За одним IP может быть несколько клиентов. Знак ≈ означает неполный снимок или ожидающие очистки подключения.</p></section>
    <section aria-label="Трафик кластера"><div className="fleet-section-heading"><h2>Трафик</h2><span>Направления относительно клиента</span></div><div className="fleet-stats three">
      <Stat label="От клиентов · Upload" icon="arrow" hint={up.hint}>{volume(up.total)}</Stat>
      <Stat label="К клиентам · Download" icon="arrow" tone="green down" hint={down.hint}>{volume(down.total)}</Stat>
      <Stat label="Общий трафик · Total" icon="globe" tone="violet" hint={up.hint}>{volume(sum(up.total, down.total))}</Stat>
    </div><p className="fleet-note">Сумма текущих счётчиков нод за их собственные периоды учёта, не календарный период. Перезапуск или смена источника учёта могут сбросить счётчики. Недоступные и устаревшие ноды не входят в сводку.</p></section>
    <section aria-label="Ресурсы кластера"><div className="fleet-section-heading"><h2>Скорость и ресурсы</h2><span>По последнему измерению</span></div><div className="fleet-stats four">
      <Stat label="Скорость Upload" icon="activity" hint={upload.hint}>{capacity(upload.total)}</Stat>
      <Stat label="Скорость Download" icon="activity" tone="green" hint={download.hint}>{capacity(download.total)}</Stat>
      <Stat label="RAM занято / всего" icon="server" tone="violet" hint={ram.hint}>{volume(ram.total)} / {volume(memory.total)}</Stat>
      <Stat label="Память процессов telemt" icon="monitor" hint={rss.hint}>{volume(rss.total)}</Stat>
    </div><p className="fleet-note">RAM — в пространстве имён ноды; несколько контейнеров на одном хосте могут видеть одну и ту же память.</p></section>
  </div>;
}

import { type ReactNode } from 'react';
import { type Node, object } from './api';
import { capacity, count, duration, percent, ratio, sum, used, volume } from './overview-model';
import { ipCount, measured } from './fleet-summary';
import { Icon } from './interface';

/** Compact rows share the same source and freshness rules as the fleet summary. */
export function ServerList({ nodes, now, actions }: { nodes: Node[]; now: number; actions: (node: Node) => ReactNode }) {
  if (!nodes.length) return <p className="empty">Серверов пока нет. Добавьте ноду по общему профилю.</p>;
  return <div className="fleet-server-list">{nodes.map(node => {
    const current = measured(node, now), snapshot = current ? node.snapshot : {}, traffic = object(snapshot.traffic), host = object(snapshot.host), info = object(snapshot.info);
    const ready = current && snapshot.ready === true;
    const status = !node.enabled ? 'Отключён' : !current ? 'Нет свежих данных' : ready ? 'Готов к работе' : 'Не готов';
    const memory = host.available === true ? ratio(used(host.memory_total_bytes, host.memory_available_bytes), host.memory_total_bytes) : null;
    const address = [node.document.ipv4, node.document.ipv6].filter(v => typeof v === 'string' && v).join(' · ');
    return <article key={node.id} aria-label={`Сервер ${node.name}`} className={`fleet-server-row ${ready ? 'ready' : 'unavailable'}`}>
      <div className="fleet-server-title"><span className="fleet-node-icon"><Icon name="server" /></span><div><h3>{node.name}</h3><span className={`fleet-node-state ${ready ? 'online' : ''}`}><i />{status}</span></div><span className="fleet-client-badge" title="Активные IP, общие для всех аккаунтов этой ноды"><Icon name="users" /><strong>{current ? ipCount(node) : '—'}</strong><small>IP</small></span></div>
      <div className="fleet-server-address"><Icon name="globe" /><code>{address || 'IP не указан'}</code></div>
      <div className="fleet-server-transfer"><span>Общий трафик</span><strong>{traffic.available === true ? volume(sum(traffic.octets_from_client, traffic.octets_to_client)) : 'Нет данных'}</strong><small>↑ {traffic.available === true ? volume(traffic.octets_from_client) : '—'} · ↓ {traffic.available === true ? volume(traffic.octets_to_client) : '—'}</small></div>
      <div className="fleet-server-uptime"><strong>{duration(info.uptime_seconds)}</strong><small>Uptime telemt{typeof info.version === 'string' ? ` · v${info.version}` : ''}</small></div>
      <div className="fleet-server-actions">{actions(node)}</div>
      <div className="fleet-server-details"><div className="fleet-row-memory"><span>RAM {percent(memory)}</span><div className="fleet-mini-gauge"><i style={{ width: `${Math.min(100, memory ?? 0)}%` }} /></div></div><span>CPU {host.available === true ? percent(host.cpu_busy_percent) : 'Нет данных'}</span><span>LA {host.available === true ? count(host.load1) : '—'}</span><span className="fleet-up">↑ {capacity(snapshot.upload_bps)}</span><span className="fleet-down">↓ {capacity(snapshot.download_bps)}</span><span>{count(traffic.current_connections)} соединений</span><span>{node.document.managed === true ? 'Общий профиль' : 'Требуется новая регистрация'}</span><span>{node.collected_at ? `Обновлено ${duration(Math.max(0, (now - Date.parse(node.collected_at)) / 1000))} назад` : 'Ожидание измерения'}</span></div>
    </article>;
  })}</div>;
}

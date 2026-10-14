import { useEffect, useState } from 'react';
import { type Json, type Node, type User, api, object, list, text, bytes, readNode } from './api';
import { Dialog, Table } from './components';
import { ManagedWizard, ManagedNodeControl } from './managed-pages';
import { ServerList } from './server-list';


export function Nodes({ nodes, user, refresh, run }: { nodes: Node[]; user: User; refresh: () => void; run: (work: () => Promise<void>) => void }) {
  const [controlled, setControlled] = useState<Node | null>(null);
  const [wizard, setWizard] = useState(false), [now, setNow] = useState(Date.now());
  useEffect(() => { const timer = setInterval(() => setNow(Date.now()), 5000); return () => clearInterval(timer); }, []);
  return <section><div className="section-title"><h2>Зарегистрированные серверы</h2>{user.role === 'owner' && <button onClick={() => setWizard(true)}>Добавить сервер</button>}</div><p>Ноды подключаются по общему профилю через выданный Fleet файл Docker Compose.</p><ServerList nodes={nodes} now={now} actions={row => user.role === 'owner' ? <>{object(row.document).managed === true && <button onClick={() => setControlled(nodes.find(n => n.id === row.id) ?? null)}>Управление</button>}<button className="danger" onClick={() => { if (confirm(`Удалить сервер ${text(row.name)} и все его зависимости в Fleet? Будут удалены его задания и история, остановлена автоматика связанных DNS-групп. Общие профили и сертификаты сохранятся. Записи Cloudflare и контейнер на сервере останутся без изменений.`)) run(async () => { await api(`nodes/${text(row.id)}`, 'DELETE', {}); refresh(); }); }}>Удалить</button></> : null} />
    {controlled && <ManagedNodeControl node={controlled} close={() => setControlled(null)} refresh={refresh} />}
    {wizard && <ManagedWizard close={() => setWizard(false)} refresh={refresh} />}
  </section>;
}

/** Node views expose runtime state; shared users are edited exclusively through profiles. */
export function Users({ nodeId, canWrite, run }: { nodeId: string; canWrite: boolean; run: (work: () => Promise<void>) => void }) {
  const [users, setUsers] = useState<Json[]>([]); const [revision, setRevision] = useState(''); const [error, setError] = useState(''); const [viewing, setViewing] = useState<Record<string, Json> | null>(null);
  useEffect(() => { let active = true; setUsers([]); setRevision(''); setViewing(null); setError(''); const fetch = () => readNode(nodeId, '/v1/users').then(response => { if (active) { setUsers(list(response.data)); setRevision(response.revision); setError(''); } }).catch(error => { if (active) setError(String(error)); }); void fetch(); const timer = setInterval(fetch, 5000); return () => { active = false; clearInterval(timer); }; }, [nodeId]);
  return <section><h2>Пользователи прокси</h2><p>Настройки и ключи меняются в общем профиле. Квоты и статистика действуют отдельно на каждой ноде.</p>{error && <p className="error">{error}</p>}<Table rows={users} columns={['username', 'enabled', 'in_runtime', 'current_connections', 'active_unique_ips', 'total_octets']} actions={row => <><button className="quiet" onClick={() => setViewing(row)}>Подробнее</button>{canWrite && <button className="quiet" disabled={!revision || !!error} onClick={() => { if (confirm(`Сбросить квоту пользователя ${text(row.username)} на этой ноде?`)) run(async () => { await api('operations', 'POST', { node_ids: [nodeId], method: 'POST', path: `/v1/users/${encodeURIComponent(text(row.username))}/reset-quota`, body: {}, revisions: { [nodeId]: revision } }); }); }}>Сбросить квоту</button>}</>} />
    {viewing && <Dialog title="Пользователь и ссылки подключения" close={() => setViewing(null)}>{list(object(viewing.links).tls).map(link => typeof link === 'string' && link.startsWith('tg://proxy?') ? <p key={link}><a href={link}>{link}</a></p> : null)}<p>Всего передано и получено: {bytes(viewing.total_octets)}</p><p>Изменение пользователя выполняется в общем профиле и применяется ко всем его нодам.</p></Dialog>}
  </section>;
}
export { Overview } from './overview-page';
export { Configuration } from './configuration';
export { Shaping } from './shaping-page';
export { Diagnostics } from './diagnostics-page';

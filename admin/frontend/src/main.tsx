import { StrictMode, useEffect, useState, useCallback } from 'react';
import { createRoot } from 'react-dom/client';
import { api, setCsrf, type User, type Node, type Operation, type Group, isStale } from './api';
import { Overview, Nodes, Users, Configuration, Shaping, Diagnostics } from './node-pages';
import { Operations, Failover, Probes, Administrators, Audit, NoNode } from './fleet-pages';
import { IPRotation } from './rotation-page';
import './styles.css';
import { Profiles, Certificates } from './managed-pages';
import { Brand, Icon, Navigation, ThemeControl, pageIcon, pageContext, useAppearance } from './interface';

const pages = ['Обзор', 'Серверы', 'Общие профили', 'Сертификаты', 'Пользователи', 'Конфигурация', 'Шейпер', 'Диагностика', 'Внешние проверки', 'Переключение DNS', 'Ротация IP', 'Операции', 'Аудит', 'Администраторы'];
function App() {
  const appearance = useAppearance();
  const [user, setUser] = useState<User | null>(null); const [checked, setChecked] = useState(false); const [error, setError] = useState(''); const [busy, setBusy] = useState(false);
  const [nodes, setNodes] = useState<Node[]>([]); const [primary, setPrimary] = useState(''); const [operations, setOperations] = useState<Operation[]>([]); const [groups, setGroups] = useState<Group[]>([]); const [page, setPage] = useState('Обзор');
  const run = useCallback((work: () => Promise<void>) => { setError(''); setBusy(true); void work().catch(error => setError(error instanceof Error ? error.message : String(error))).finally(() => setBusy(false)); }, []);
  const refresh = useCallback(async () => { const [nextNodes, nextOperations, nextGroups] = await Promise.all([api<Node[]>('nodes'), api<Operation[]>('operations'), api<Group[]>('groups')]); setNodes(nextNodes); setOperations(nextOperations); setGroups(nextGroups); setPrimary(current => nextNodes.some(node => node.id === current) ? current : nextNodes[0]?.id || ''); }, []);
  useEffect(() => { const expired = () => { setUser(null); setCsrf(''); }; window.addEventListener('telemt-session-expired', expired); return () => window.removeEventListener('telemt-session-expired', expired); }, []);
  useEffect(() => { api<{ user: User; csrf: string }>('session').then(session => { setUser(session.user); setCsrf(session.csrf); }).catch(() => undefined).finally(() => setChecked(true)); }, []);
  useEffect(() => { if (!user) return; run(refresh); const timer = setInterval(() => void refresh().catch(error => setError(String(error))), 5000); return () => clearInterval(timer); }, [user, refresh, run]);
  if (!checked) return <div className="login"><p>Загрузка кабинета…</p></div>;
  if (!user) return <div className="login"><form className="login-card" onSubmit={event => { event.preventDefault(); const data = new FormData(event.currentTarget); run(async () => { const session = await api<{ user: User; csrf: string }>('login', 'POST', { username: String(data.get('username')), password: String(data.get('password')), totp: String(data.get('totp')) }); setUser(session.user); setCsrf(session.csrf); }); }}><Brand /><h1>Вход администратора</h1><label>Имя пользователя<input name="username" autoComplete="username" required /></label><label>Пароль<input name="password" type="password" autoComplete="current-password" required /></label><label>Код из приложения-аутентификатора<input name="totp" inputMode="numeric" pattern="[0-9]{6}" maxLength={6} autoComplete="one-time-code" required /></label>{error && <p role="alert" className="error">{error}</p>}<button disabled={busy}>{busy ? 'Проверка…' : 'Войти'}</button><p className="muted">Независимые ноды · централизованное управление</p></form></div>;
  const node = nodes.find(node => node.id === primary); const canWrite = user.role !== 'viewer';
  const context = pageContext(page);
  const nodeScoped = ['Обзор', 'Пользователи', 'Конфигурация', 'Шейпер', 'Диагностика'].includes(page);
  return <div className={`layout${appearance.collapsed ? ' is-collapsed' : ''}`}><Navigation pages={pages} page={page} select={setPage} user={user} collapsed={appearance.collapsed} toggle={() => appearance.setCollapsed(value => !value)} logout={() => run(async () => { await api('logout', 'POST', {}); setUser(null); setCsrf(''); })} /><main><header className="app-header"><div className="page-heading"><span className="page-icon"><Icon name={pageIcon(page)} /></span><div className="page-title"><div className="breadcrumb">Fleet <span>/</span> {context.group}</div><h1>{page}</h1></div></div><div className="actions"><span className="refresh-status"><span className="live-dot" /> Каждые 5 с</span><ThemeControl theme={appearance.theme} change={appearance.setTheme} /><button className="quiet refresh-button" disabled={busy} onClick={() => run(refresh)}><Icon name="rotate" /><span>Обновить</span></button></div></header><div className="page-intro"><p>{context.description}</p><span className="workspace-tag">Панель управления</span></div>{error && <div className="banner error" role="alert">{error}<button className="quiet" onClick={() => setError('')}>Скрыть</button></div>}
    {nodeScoped && nodes.length > 0 && <div className="targets"><label>Просматриваемый сервер<select aria-label="Просматриваемый сервер" value={primary} onChange={event => setPrimary(event.target.value)}>{nodes.map(node => <option value={node.id} key={node.id}>{node.name}</option>)}</select></label>{node && isStale(node) && <span className="stale">Данные мониторинга устарели</span>}</div>}
    {page === 'Обзор' && <Overview nodes={nodes} nodeId={primary} inspect={setPrimary} />}{page === 'Серверы' && <Nodes nodes={nodes} user={user} refresh={() => run(refresh)} run={run} />}
    {['Пользователи', 'Конфигурация', 'Шейпер', 'Диагностика'].includes(page) && !node && <NoNode />}
    {node && ['Пользователи', 'Конфигурация', 'Шейпер'].includes(page) && <p>Эта нода управляется общим профилем. Меняйте пользователей и настройки в разделе «Общие профили».</p>}
    {node && page === 'Пользователи' && <Users key={node.id} nodeId={node.id} canWrite={canWrite} run={run} />}
    {node && page === 'Конфигурация' && <Configuration key={node.id} nodeId={node.id}  />}
    {node && page === 'Шейпер' && <Shaping key={node.id} nodeId={node.id}  />}
    {node && page === 'Диагностика' && <Diagnostics key={node.id} nodeId={node.id} canWrite={canWrite} run={run} />}
    {page === 'Общие профили' && <Profiles user={user} groups={groups} nodes={nodes} run={run} />}{page === 'Сертификаты' && <Certificates user={user} run={run} />}
    {page === 'Внешние проверки' && <Probes user={user} run={run} nodes={nodes} />}{page === 'Переключение DNS' && <Failover nodes={nodes} groups={groups} user={user} run={run} refresh={() => run(refresh)} />}
    {page === 'Ротация IP' && <IPRotation nodes={nodes} groups={groups} user={user} run={run} />}
    {page === 'Операции' && <Operations rows={operations} run={run} refresh={() => run(refresh)} />}{page === 'Аудит' && <Audit run={run} />}{page === 'Администраторы' && user.role === 'owner' && <Administrators run={run} />}
    <footer><span>telemt <b>Fleet</b></span><span>Ноды работают автономно при недоступности кабинета.</span></footer></main></div>;
}
createRoot(document.getElementById('root')!).render(<StrictMode><App /></StrictMode>);

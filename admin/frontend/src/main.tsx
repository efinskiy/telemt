import { StrictMode, useEffect, useState, useCallback } from 'react';
import { createRoot } from 'react-dom/client';
import { api, setCsrf, readNode, type Json, type User, type Node, type Operation, type Group, isStale } from './api';
import { Overview, Nodes, Users, Configuration, Shaping, Diagnostics } from './node-pages';
import { Operations, Failover, Probes, Administrators, Audit, NoNode } from './fleet-pages';
import { IPRotation } from './rotation-page';
import './styles.css';

const pages = ['Overview', 'Servers', 'Users', 'Configuration', 'Shaping', 'Diagnostics', 'External probes', 'DNS failover', 'IP rotation', 'Operations', 'Audit', 'Administrators'];
function App() {
  const [user, setUser] = useState<User | null>(null); const [checked, setChecked] = useState(false); const [error, setError] = useState(''); const [notice, setNotice] = useState(''); const [busy, setBusy] = useState(false);
  const [nodes, setNodes] = useState<Node[]>([]); const [selected, setSelected] = useState<string[]>([]); const [primary, setPrimary] = useState(''); const [operations, setOperations] = useState<Operation[]>([]); const [groups, setGroups] = useState<Group[]>([]); const [page, setPage] = useState('Overview');
  const run = useCallback((work: () => Promise<void>) => { setError(''); setBusy(true); void work().catch(error => setError(error instanceof Error ? error.message : String(error))).finally(() => setBusy(false)); }, []);
  const refresh = useCallback(async () => { const [nextNodes, nextOperations, nextGroups] = await Promise.all([api<Node[]>('nodes'), api<Operation[]>('operations'), api<Group[]>('groups')]); setNodes(nextNodes); setOperations(nextOperations); setGroups(nextGroups); setPrimary(current => nextNodes.some(node => node.id === current) ? current : nextNodes[0]?.id || ''); setSelected(current => current.length ? current.filter(id => nextNodes.some(node => node.id === id)) : nextNodes.slice(0, 1).map(node => node.id)); }, []);
  useEffect(() => { const expired = () => { setUser(null); setCsrf(''); }; window.addEventListener('telemt-session-expired', expired); return () => window.removeEventListener('telemt-session-expired', expired); }, []);
  useEffect(() => { api<{ user: User; csrf: string }>('session').then(session => { setUser(session.user); setCsrf(session.csrf); }).catch(() => undefined).finally(() => setChecked(true)); }, []);
  useEffect(() => { if (!user) return; run(refresh); const timer = setInterval(() => void refresh().catch(error => setError(String(error))), 5000); return () => clearInterval(timer); }, [user, refresh, run]);
  const submit = async (method: string, path: string, body: Json, supplied?: Record<string, string>) => {
    if (!selected.length) throw new Error('Select at least one target server');
    const revisions = supplied ?? {}; for (const id of selected) if (!revisions[id]) revisions[id] = (await readNode(id, '/v1/config')).revision;
    const operation = await api<{ id: string }>('operations', 'POST', { node_ids: selected, method, path, body, revisions }); setNotice(`Operation ${operation.id} submitted. Follow its actual completion in Operations.`); await refresh();
  };
  if (!checked) return <div className="login"><p>Opening control plane…</p></div>;
  if (!user) return <div className="login"><form className="login-card" onSubmit={event => { event.preventDefault(); const data = new FormData(event.currentTarget); run(async () => { const session = await api<{ user: User; csrf: string }>('login', 'POST', { username: String(data.get('username')), password: String(data.get('password')), totp: String(data.get('totp')) }); setUser(session.user); setCsrf(session.csrf); }); }}><div className="brand">TELEMT<span>CONTROL PLANE</span></div><h1>Administrator sign in</h1><label>Username<input name="username" autoComplete="username" required /></label><label>Password<input name="password" type="password" autoComplete="current-password" required /></label><label>Authenticator code<input name="totp" inputMode="numeric" pattern="[0-9]{6}" maxLength={6} autoComplete="one-time-code" required /></label>{error && <p role="alert" className="error">{error}</p>}<button disabled={busy}>{busy ? 'Verifying…' : 'Sign in'}</button><p className="muted">Independent traffic nodes · authenticated fleet control</p></form></div>;
  const node = nodes.find(node => node.id === primary); const canWrite = user.role !== 'viewer';
  return <div className="layout"><aside><div className="brand">TELEMT<span>CONTROL PLANE</span></div><nav>{pages.filter(item => item !== 'Administrators' || user.role === 'owner').map(item => <button key={item} className={page === item ? 'nav-active' : ''} onClick={() => setPage(item)}>{item}</button>)}</nav><div className="account"><strong>{user.username}</strong><span>{user.role}</span><button className="quiet" onClick={() => run(async () => { await api('logout', 'POST', {}); setUser(null); setCsrf(''); setNotice(''); })}>Sign out</button></div></aside><main><header><div><p className="eyebrow">FLEET OPERATIONS</p><h1>{page}</h1></div><div className="actions"><span className="live-dot" /> <span className="muted">5-second collection</span><button className="quiet" disabled={busy} onClick={() => run(refresh)}>Refresh</button></div></header>{error && <div className="banner error" role="alert">{error}<button className="quiet" onClick={() => setError('')}>Dismiss</button></div>}{notice && <div className="banner" role="status">{notice}<button className="quiet" onClick={() => setNotice('')}>Dismiss</button></div>}
    {nodes.length > 0 && <div className="targets"><label>Inspect server<select value={primary} onChange={event => setPrimary(event.target.value)}>{nodes.map(node => <option value={node.id} key={node.id}>{node.name}</option>)}</select></label>{canWrite && <fieldset><legend>Mutation targets</legend>{nodes.map(node => <label key={node.id}><input type="checkbox" checked={selected.includes(node.id)} onChange={event => setSelected(event.target.checked ? [...selected, node.id] : selected.filter(id => id !== node.id))} />{node.name}</label>)}</fieldset>}{node && isStale(node) && <span className="stale">Monitoring data is stale</span>}</div>}
    {page === 'Overview' && <Overview nodes={nodes} />}{page === 'Servers' && <Nodes nodes={nodes} user={user} refresh={() => run(refresh)} run={run} />}
    {['Users', 'Configuration', 'Shaping', 'Diagnostics'].includes(page) && !node && <NoNode />}
    {node && page === 'Users' && <Users key={node.id} nodeId={node.id} canWrite={canWrite} submit={submit} run={run} />}
    {node && page === 'Configuration' && <Configuration key={node.id} nodeId={node.id} selected={selected} canWrite={canWrite} submit={submit} />}
    {node && page === 'Shaping' && <Shaping key={node.id} nodeId={node.id} canWrite={canWrite} submit={submit} run={run} />}
    {node && page === 'Diagnostics' && <Diagnostics key={node.id} nodeId={node.id} canWrite={canWrite} run={run} />}
    {page === 'External probes' && <Probes user={user} run={run} />}{page === 'DNS failover' && <Failover nodes={nodes} groups={groups} user={user} run={run} refresh={() => run(refresh)} />}
    {page === 'IP rotation' && <IPRotation nodes={nodes} groups={groups} user={user} run={run} />}
    {page === 'Operations' && <Operations rows={operations} run={run} refresh={() => run(refresh)} />}{page === 'Audit' && <Audit run={run} />}{page === 'Administrators' && user.role === 'owner' && <Administrators run={run} />}
    <footer>Control plane outages do not stop local proxy traffic or shaping.</footer></main></div>;
}
createRoot(document.getElementById('root')!).render(<StrictMode><App /></StrictMode>);

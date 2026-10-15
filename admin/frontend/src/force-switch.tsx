import { useEffect, useState } from 'react';
import { api, object, text, type Group, type Json, type Node } from './api';
import { Dialog, Empty } from './components';

/** Keeps confirmation bound to the exact DNS preview returned by Fleet. */
export function ForceSwitch({ group, nodes, close, refresh }: { group: Group; nodes: Node[]; close: () => void; refresh: () => void }) {
  const [target, setTarget] = useState('');
  const [preview, setPreview] = useState<Json>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const members = nodes.filter(node => node.enabled && node.id !== group.active_node && Array.isArray(group.document.node_ids) && group.document.node_ids.includes(node.id));
  const available = members.some(node => node.id === target);
  const groupState = JSON.stringify([group.active_node, group.document, group.observed]);
  useEffect(() => { setPreview(null); }, [groupState, target, available]);
  const selected = object(preview);
  const name = (id: Json | undefined) => nodes.find(node => node.id === id)?.name ?? (id ? text(id) : 'Не назначен');
  async function submit() {
    setBusy(true); setError('');
    try {
      if (preview) {
        await api(`groups/${group.id}/force-switch`, 'POST', { node_id: target, preview });
        refresh(); close();
      } else {
        setPreview(await api<Json>(`groups/${group.id}/force-preview`, 'POST', { node_id: target }));
      }
    } catch (error) { setPreview(null); setError(String(error)); refresh(); }
    finally { setBusy(false); }
  }
  return <Dialog title={`Сменить активный сервер · ${group.name}`} close={() => { if (!busy) close(); }}>
    <p>Сейчас активен: <strong>{name(group.active_node)}</strong></p>
    <p>Ручной выбор обходит кворум внешних проверок, ожидание 5 минут и отказ всех IP текущего сервера. Fleet проверит готовность и совместимость выбранной ноды.</p>
    <label className="label">Новый активный сервер<select disabled={busy} value={target} onChange={event => { setTarget(event.target.value); setError(''); }}><option value="">Выберите сервер</option>{members.map(node => <option key={node.id} value={node.id}>{node.name} · {text(node.document.ipv4 || node.document.ipv6)}</option>)}</select></label>
    {!members.length && <Empty>В группе нет другого включённого сервера. Добавьте резерв того же профиля.</Empty>}
    {preview && <><h3>{name(selected.old_node)} → {name(selected.node_id)}</h3><div className="table-wrap"><table><thead><tr><th>Запись</th><th>Текущий IP</th><th>Новый IP</th></tr></thead><tbody>{Object.entries(object(selected.intent)).map(([family, value]) => <tr key={family}><td>{family}</td><td>{text(object(object(selected.observed)[family]).content)}</td><td>{text(object(value).content)}</td></tr>)}</tbody></table></div><p>DNS-only · TTL 60 секунд. Открытые соединения останутся на прежней ноде; новые подключения перейдут после обновления DNS-кеша клиентов.</p></>}
    <p>При переключении автоматика группы и всех её IP-пулов будет выключена. Включить её снова можно вручную.</p>
    {error && <p role="alert" className="error">{error}</p>}
    <div className="actions"><button disabled={busy || !available} onClick={() => void submit()}>{busy ? 'Выполняется…' : preview ? 'Подтвердить переключение' : 'Проверить и показать DNS'}</button><button className="quiet" disabled={busy} onClick={close}>Отмена</button></div>
  </Dialog>;
}

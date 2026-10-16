import { useEffect, useState } from 'react';
import { api } from './api';
import { errorLabel } from './labels';

type Settings = { revision: number; enabled: boolean; chat_id: string; token_configured: boolean; ip_rotation: boolean; server_switch: boolean };
type Delivery = { id: string; kind: string; status: string; attempts: number; error: string | null; created_at: string; message: string };
const kinds: Record<string, string> = { ip_rotation: 'Ротация IP', server_switch: 'Смена сервера', test: 'Проверка доставки' };
const statuses: Record<string, string> = { queued: 'В очереди', sent: 'Доставлено', failed: 'Ошибка', cancelled: 'Отменено' };

/** Credentials are write-only; polling refreshes history without replacing unsaved form edits. */
export function Notifications() {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [token, setToken] = useState(''); const [clearToken, setClearToken] = useState(false);
  const [history, setHistory] = useState<Delivery[]>([]);
  const [busy, setBusy] = useState(false); const [dirty, setDirty] = useState(false);
  const [error, setError] = useState(''); const [notice, setNotice] = useState('');
  useEffect(() => {
    let active = true;
    void api<Settings>('notifications').then(value => { if (active) setSettings(value); }).catch(e => { if (active) setError(String(e)); });
    const refresh = () => void api<Delivery[]>('notifications/history').then(value => { if (active) setHistory(value); }).catch(e => { if (active) setError(String(e)); });
    refresh(); const timer = setInterval(refresh, 5000);
    return () => { active = false; clearInterval(timer); };
  }, []);
  const update = (value: Partial<Settings>) => { setSettings(current => current && { ...current, ...value }); setDirty(true); setNotice(''); };
  const run = async (work: () => Promise<void>) => { setBusy(true); setError(''); setNotice(''); try { await work(); } catch (e) { setError(String(e)); } finally { setBusy(false); } };
  return <div className="notification-page">
    {error && <p className="error" role="alert">{error}</p>}
    {notice && <p role="status">{notice}</p>}
    {settings && <section className="card"><h2>Уведомления в Telegram</h2>
      <p className="muted">Сообщения о подтверждённой смене DNS для всех групп Fleet. Создайте бота через @BotFather. Для личного чата сначала отправьте боту /start; для группы добавьте бота и разрешите отправку сообщений.</p>
      <form onSubmit={event => { event.preventDefault(); void run(async () => {
        const saved = await api<Settings>('notifications', 'PUT', { ...settings, bot_token: token, clear_token: clearToken });
        setSettings(saved); setToken(''); setClearToken(false); setDirty(false); setNotice('Настройки сохранены.');
        setHistory(await api<Delivery[]>('notifications/history'));
      }); }}>
        <fieldset disabled={busy}><div className="form-grid">
          <label>Получатель (chat ID)<input aria-label="Получатель (chat ID)" value={settings.chat_id} maxLength={128} placeholder="123456789 или -1001234567890" onChange={e => update({ chat_id: e.target.value })} /><small>Числовой ID личного чата или группы. Для публичного канала можно указать @имя_канала.</small></label>
          <label>Токен бота<input aria-label="Токен бота" type="password" autoComplete="new-password" value={token} maxLength={256} placeholder={settings.token_configured ? 'Токен сохранён' : 'Токен от @BotFather'} onChange={e => { setToken(e.target.value); setClearToken(false); setDirty(true); }} /><small>Хранится зашифрованным. Пустое поле сохраняет текущий токен.</small></label>
        </div>
        <label><input type="checkbox" checked={settings.enabled} onChange={e => update({ enabled: e.target.checked })} /> Включить уведомления</label>
        <label><input type="checkbox" checked={settings.ip_rotation} onChange={e => update({ ip_rotation: e.target.checked })} /> Ротация IP на сервере</label>
        <label><input type="checkbox" checked={settings.server_switch} onChange={e => update({ server_switch: e.target.checked })} /> Смена активного сервера</label>
        {settings.token_configured && <label><input type="checkbox" checked={clearToken} onChange={e => { setClearToken(e.target.checked); setToken(''); update({ enabled: false }); }} /> Удалить сохранённый токен</label>}
        <p className="muted">Сохранение отменяет ещё не отправленные уведомления с прежними настройками. Новые события сохраняются в очереди; при временной ошибке доставка повторяется до 8 раз в течение суток.</p>
        <div className="actions"><button type="submit">Сохранить</button><button type="button" className="quiet" disabled={dirty || !settings.token_configured || !settings.chat_id} onClick={() => void run(async () => { await api('notifications/test', 'POST', {}); setNotice('Проверочное сообщение поставлено в очередь. Результат появится в истории доставки.'); setHistory(await api<Delivery[]>('notifications/history')); })}>Отправить проверку</button></div>
        {dirty && <small>Сохраните изменения перед проверкой доставки.</small>}
        </fieldset>
      </form>
    </section>}
    <section className="card"><h2>История доставки</h2><p className="muted">Последние 50 сообщений за 30 дней. При потере ответа Telegram возможна повторная доставка.</p>
      {!history.length ? <p>Уведомлений пока нет.</p> : <div className="table-wrap"><table><thead><tr><th>Время</th><th>Событие</th><th>Доставка</th><th>Попытки</th><th>Подробности</th></tr></thead><tbody>{history.map(row => <tr key={row.id}>
        <td>{new Date(row.created_at).toLocaleString('ru-RU')}</td><td>{kinds[row.kind] ?? row.kind}</td><td>{statuses[row.status] ?? row.status}</td><td>{row.attempts}</td>
        <td>{row.error && <p>{errorLabel(row.error)}</p>}<details><summary>Сообщение</summary>{row.message.split('\n').map((line, i) => <div key={i}>{line}</div>)}</details></td>
      </tr>)}</tbody></table></div>}
    </section>
  </div>;
}

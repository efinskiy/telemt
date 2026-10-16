import { useEffect, useState } from 'react';
import { api } from './api';
import { date, errorLabel, stateLabel } from './labels';

type Binding = { telegram_id: string; administrator_id: string };
type Settings = { enabled: boolean; revision: number; token_configured: boolean; username: string | null; last_poll_at: string | null; last_error: string | null; bindings: Binding[] };
type Administrator = { id: string; username: string; role: string; enabled: boolean };

/** The notification credential is shared; Telegram access remains an independent, explicit grant. */
export function TelegramBot({ openNotifications }: { openNotifications: () => void }) {
  const [settings, setSettings] = useState<Settings | null>(null), [runtime, setRuntime] = useState<Settings | null>(null);
  const [admins, setAdmins] = useState<Administrator[]>([]), [busy, setBusy] = useState(false), [error, setError] = useState(''), [saved, setSaved] = useState(false);
  useEffect(() => {
    let active = true;
    void Promise.all([api<Settings>('telegram-bot'), api<Administrator[]>('administrators')]).then(([value, users]) => { if (active) { setSettings(value); setRuntime(value); setAdmins(users); } }).catch(e => { if (active) setError(String(e)); });
    const timer = setInterval(() => void api<Settings>('telegram-bot').then(value => { if (active) setRuntime(value); }).catch(e => { if (active) setError(String(e)); }), 5000);
    return () => { active = false; clearInterval(timer); };
  }, []);
  const update = (change: Partial<Settings>) => { setSettings(current => current && { ...current, ...change }); setSaved(false); };
  const editBinding = (index: number, change: Partial<Binding>) => update({ bindings: settings!.bindings.map((binding, i) => i === index ? { ...binding, ...change } : binding) });
  const fresh = runtime?.last_poll_at && Date.now() - Date.parse(runtime.last_poll_at) < 60000;
  return <div className="notification-page">
    {error && <p role="alert" className="error">{error}</p>}{saved && <p role="status">Настройки Telegram-бота сохранены.</p>}
    <section className="card"><h2>Управление Fleet через Telegram</h2>
      <p>Используется тот же бот, что и для уведомлений. Получатель уведомлений не получает права управления автоматически.</p>
      <div className="actions"><span>{runtime?.token_configured ? 'Токен настроен' : 'Токен не настроен'}</span><button className="quiet" onClick={openNotifications}>Настроить токен в уведомлениях</button>
        {runtime?.username && /^[A-Za-z0-9_]+$/.test(runtime.username) && <a href={`https://t.me/${runtime.username}`} target="_blank" rel="noreferrer">Открыть @{runtime.username}</a>}</div>
      <p>Состояние: {runtime?.enabled ? runtime.last_error ? 'Ошибка подключения' : fresh ? 'Принимает команды' : 'Ожидает подключения к Telegram' : 'Управление выключено'}. Последний опрос: {runtime?.last_poll_at ? date(runtime.last_poll_at) : 'ещё не выполнялся'}.</p>
      {runtime?.last_error && <p className="error">{runtime.last_error === 'telegram_rate_limited' ? 'Telegram ограничил частоту запросов. Бот ждёт разрешённого времени повторного опроса; результат команды проверьте в истории операций.' : errorLabel(runtime.last_error)}</p>}
      {settings && <form onSubmit={event => { event.preventDefault(); setBusy(true); setError(''); setSaved(false); void api<Settings>('telegram-bot', 'PUT', { revision: settings.revision, enabled: settings.enabled, bindings: settings.bindings.map(b => ({ telegram_id: b.telegram_id.trim(), administrator_id: b.administrator_id })) }).then(value => { setSettings(value); setRuntime(value); setSaved(true); }).catch(e => setError(String(e))).finally(() => setBusy(false)); }}>
        <fieldset disabled={busy}>
          <label><input type="checkbox" checked={settings.enabled} onChange={e => update({ enabled: e.target.checked })} /> Включить управление через Telegram</label>
          <h3>Разрешённые пользователи</h3><p className="muted">Привяжите числовой Telegram ID к администратору Fleet. Его текущие права применяются при каждой команде. Только владелец может менять DNS, авторотацию и добавлять ноды; остальные роли получают аналитику.</p>
          {settings.bindings.map((binding, index) => <div className="telegram-access-row" key={index}>
            <label>Telegram ID<input aria-label={`Telegram ID ${index + 1}`} inputMode="numeric" pattern="[1-9][0-9]{0,15}" maxLength={16} required value={binding.telegram_id} onChange={e => editBinding(index, { telegram_id: e.target.value })} /></label>
            <label>Администратор Fleet<select aria-label={`Администратор Fleet ${index + 1}`} value={binding.administrator_id} required onChange={e => editBinding(index, { administrator_id: e.target.value })}><option value="" disabled>Выберите администратора</option>{admins.map(admin => <option key={admin.id} value={admin.id} disabled={!admin.enabled}>{admin.username} · {stateLabel(admin.role)}{admin.enabled ? '' : ' · отключён'}</option>)}</select></label>
            <button type="button" className="quiet" aria-label={`Удалить доступ ${index + 1}`} onClick={() => update({ bindings: settings.bindings.filter((_, i) => i !== index) })}>Удалить</button>
          </div>)}
          <div className="actions"><button type="button" className="quiet" disabled={settings.bindings.length >= 100} onClick={() => update({ bindings: [...settings.bindings, { telegram_id: '', administrator_id: '' }] })}>Добавить доступ</button><button type="submit">Сохранить</button></div>
          <small>Сохранение отменяет незавершённые подтверждения. Команды, отправленные до включения бота, не исполняются.</small>
        </fieldset>
      </form>}
    </section>
    <section className="card"><h2>Как пользоваться</h2><ol>
      <li>Настройте токен в разделе «Уведомления», добавьте свой Telegram ID и включите управление.</li>
      <li>Дождитесь состояния «Принимает команды» и отправьте боту <code>/start</code>. Команда <code>/id</code> показывает ваш Telegram ID.</li>
      <li>Откройте сводку, сервер, здоровье IP или страны клиентов. Управление доступно кнопками и подтверждается отдельно.</li>
      <li>Для новой ноды выберите профиль, введите имя и IP. После подтверждения бот пришлёт <code>docker-compose.yml</code>; сохраните его на ноде и запустите <code>docker compose up -d</code>.</li>
    </ol><p className="muted">Rich Messages Bot API 10.3: компактные таблицы, раскрываемые пояснения, кнопки и локальное время. Используйте актуальную версию Telegram. Доступ работает только в личном чате. Этим токеном не должен пользоваться другой получатель команд или webhook.</p>
    <p className="muted">Бот получает обновления сам — отдельный сервер и публичный webhook не нужны. Автоматические уведомления работают независимо от переключателя управления.</p></section>
  </div>;
}

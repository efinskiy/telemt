import { useState } from 'react';
import { api, type Json, type Node } from './api';

/** Address edits queue an immutable deployment and never implicitly rewrite public DNS. */
export function NodeAddresses({ node, refresh }: { node: Node; refresh: () => void }) {
  const [ipv4, setIpv4] = useState(String(node.document.ipv4 ?? ''));
  const [ipv6, setIpv6] = useState(String(node.document.ipv6 ?? ''));
  const [expected, setExpected] = useState<Json>({ ipv4: node.document.ipv4 ?? null, ipv6: node.document.ipv6 ?? null });
  const [busy, setBusy] = useState(false), [error, setError] = useState(''), [notice, setNotice] = useState('');
  return <form onSubmit={async event => {
    event.preventDefault(); setBusy(true); setError(''); setNotice('');
    try {
      const result = await api<{ addresses: { ipv4: string | null; ipv6: string | null }; deployment: number }>(`nodes/${node.id}/addresses`, 'PATCH', { ipv4: ipv4.trim() || null, ipv6: ipv6.trim() || null, expected_addresses: expected });
      setExpected(result.addresses); setIpv4(result.addresses.ipv4 ?? ''); setIpv6(result.addresses.ipv6 ?? '');
      setNotice(`IP сохранён. Конфигурация №${result.deployment} поставлена в очередь; нода получит её при следующем подключении к Fleet. DNS не изменён, автоматика связанных групп и IP-пулов выключена.`); refresh();
    } catch (error) { setError(String(error)); }
    finally { setBusy(false); }
  }}><h3>IP-адреса сервера</h3><p>Укажите адреса, уже назначенные серверу в панели хостинга и его интерфейсам. Можно сохранить их, даже когда нода недоступна. Хотя бы один адрес обязателен.</p>
    <div className="form-grid"><label className="label">IPv4 сервера<input value={ipv4} maxLength={45} disabled={busy} onChange={event => setIpv4(event.target.value)} placeholder="203.0.113.10" /></label><label className="label">IPv6 сервера<input value={ipv6} maxLength={45} disabled={busy} onChange={event => setIpv6(event.target.value)} placeholder="2001:db8::10" /></label></div>
    <p className="field-help">Сохранение обновит слушатели telemt и nginx. Старый адрес в IP-пуле будет заменён; применение перезапускает telemt и разрывает открытые соединения. Общий профиль, ключи и сертификат сохраняются. После применения обновите DNS отдельным действием «Сменить активный сервер».</p>
    {error && <p role="alert" className="error">{error}</p>}{notice && <p role="status">{notice}</p>}
    <button disabled={busy || (!ipv4.trim() && !ipv6.trim())}>{busy ? 'Сохраняется…' : 'Сохранить IP и применить на ноде'}</button>
  </form>;
}

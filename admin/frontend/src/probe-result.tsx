import { type Json, object } from './api';
import { Badge } from './components';

const stages: Record<string, string> = { ok: 'TLS установлен', wrong_version_number: 'WRONG_VERSION_NUMBER — IP ответил', refused: 'TCP-соединение отклонено', tls: 'Другая ошибка TLS', target: 'Некорректная цель', tcp: 'Соединение TCP', fake_tls: 'Рукопожатие FakeTLS', mtproto: 'Обмен MTProto', web: 'Проверка WEB', timeout: 'Время ожидания истекло', expired: 'Задание просрочено', network_unknown: 'Сеть агента не подтверждена' };
/** Port-22 evidence is independent of historical WEB/MTProxy reports. */
export function ProbeSignal({ row, protocol }: { row: Record<string, Json>; protocol: 'tcp22' }) {
  const result = object(row.result), value = object(result[protocol]);
  const unknown = result.check !== 'tcp_tls22' || result.network_ok !== true || result.stage === 'expired' || value.reachable !== true && value.stage !== 'timeout';
  const state = unknown ? 'unknown' : value.reachable === true ? 'reachable' : value.reachable === false ? 'unreachable' : 'unknown';
  const stage = result.network_ok === false ? 'network_unknown' : result.stage === 'expired' ? 'expired' : value.stage;
  const latency = typeof value.latency_ms === 'number' && Number.isFinite(value.latency_ms) && value.latency_ms >= 0 ? value.latency_ms : null;
  return <div className="probe-signal">{state === 'unknown' ? <span className="badge">Неизвестно</span> : <Badge value={state} />}{state !== 'unknown' && latency !== null && <small>{latency.toLocaleString('ru-RU')} мс</small>}{stage && stage !== 'ok' && <small>{stages[String(stage)] ?? String(stage)}</small>}</div>;
}

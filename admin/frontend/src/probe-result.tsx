import { type Json, object } from './api';
import { Badge } from './components';

const stages: Record<string, string> = { ok: 'Проверка пройдена', tcp: 'Соединение TCP', fake_tls: 'Рукопожатие FakeTLS', mtproto: 'Обмен MTProto', web: 'Проверка WEB', timeout: 'Время ожидания истекло', expired: 'Задание просрочено', network_unknown: 'Сеть агента не подтверждена' };
/** A failed reference or unchecked WEB result is unknown, never evidence of a blocked node. */
export function ProbeSignal({ row, protocol }: { row: Record<string, Json>; protocol: 'native' | 'web' }) {
  const result = object(row.result), value = object(result[protocol]);
  const unknown = result.network_ok === false || result.stage === 'expired' || protocol === 'web' && result.web_checked === false;
  const state = unknown ? 'unknown' : value.reachable === true ? 'reachable' : value.reachable === false ? 'unreachable' : 'unknown';
  const stage = result.network_ok === false ? 'network_unknown' : result.stage === 'expired' ? 'expired' : value.stage;
  const latency = typeof value.latency_ms === 'number' && Number.isFinite(value.latency_ms) && value.latency_ms >= 0 ? value.latency_ms : null;
  return <div className="probe-signal">{state === 'unknown' ? <span className="badge">Неизвестно</span> : <Badge value={state} />}{state !== 'unknown' && latency !== null && <small>{latency.toLocaleString('ru-RU')} мс</small>}{stage && stage !== 'ok' && <small>{stages[String(stage)] ?? String(stage)}</small>}</div>;
}

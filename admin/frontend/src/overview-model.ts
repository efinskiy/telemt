import { type Json, type Node, object, list } from './api';
export const value = (v: Json | undefined): number | null => typeof v === 'number' && Number.isFinite(v) && v >= 0 ? v : null;
export const count = (v: Json | undefined) => value(v) === null ? 'Нет данных' : Number(v).toLocaleString('ru-RU', { maximumFractionDigits: 1 });
export const percent = (v: Json | undefined) => value(v) === null ? 'Нет данных' : `${count(v)}%`;
export const capacity = (v: Json | undefined) => value(v) === null ? 'Нет данных' : `${(Number(v) / 1e6).toLocaleString('ru-RU', { maximumFractionDigits: 1 })} Мбит/с`;
export function volume(v: Json | undefined) {
  const n = value(v); if (n === null) return 'Нет данных';
  const i = n >= 1024 ** 3 ? 3 : n >= 1024 ** 2 ? 2 : n >= 1024 ? 1 : 0;
  return `${(n / 1024 ** i).toLocaleString('ru-RU', { maximumFractionDigits: 1 })} ${['Б', 'КиБ', 'МиБ', 'ГиБ'][i]}`;
}
export function duration(v: Json | undefined) {
  const n = value(v); if (n === null) return 'Нет данных';
  return n >= 86400 ? `${count(n / 86400)} д` : n >= 3600 ? `${count(n / 3600)} ч` : n >= 60 ? `${count(n / 60)} мин` : `${count(n)} с`;
}
export const sum = (a: Json | undefined, b: Json | undefined) => value(a) !== null && value(b) !== null ? Number(a) + Number(b) : null;
export const used = (total: Json | undefined, available: Json | undefined) => value(total) !== null && value(available) !== null && Number(total) >= Number(available) ? Number(total) - Number(available) : null;
export const ratio = (n: Json | undefined, total: Json | undefined) => value(n) !== null && value(total) !== null && Number(total) > 0 ? Number(n) * 100 / Number(total) : null;
export function fresh(node: Node, now = Date.now()) { const time = Date.parse(node.collected_at ?? ''); return node.enabled && Number.isFinite(time) && now - time <= 15000 && now >= time - 5000; }
export function dcRows(node: Node) { return list(object(node.snapshot.dcs).dcs).map(object).slice(0, 64); }
export function dcFresh(node: Node, now = Date.now()) { const at = value(object(node.snapshot.dcs).generated_at_epoch_secs); return fresh(node, now) && at !== null && Math.abs(now - at * 1000) <= 30000; }
export function bulkLimit(policy: Record<string, Json>, direction: 'up' | 'down', idle = false) {
  const total = value(policy[`${direction}_bps`]); if (total === null) return null;
  const cap = value(policy[`${idle ? 'bulk_idle' : 'bulk'}_${direction}_bps`]);
  const share = value(policy.priority_percent);
  if (idle) return cap ? Math.min(cap, total) : total;
  if (share === null) return null;
  const remainder = total * (100 - share) / 100;
  return cap ? Math.min(cap, remainder) : remainder;
}
export type Alert = { node: Node; severity: 'error' | 'warning'; message: string };
/** Thresholds match the operator-visible labels; unknown measurements never become healthy zeroes. */
export function alerts(nodes: Node[], now: number): Alert[] {
  return nodes.flatMap(node => {
    if (!node.enabled) return [];
    const add = (severity: Alert['severity'], message: string): Alert => ({ node, severity, message });
    if (!fresh(node, now)) return [add('warning', 'Нет свежих измерений более 15 секунд')];
    if (node.snapshot.management !== 'reachable') return [add('error', 'API управления недоступен')];
    const result: Alert[] = [], host = object(node.snapshot.host);
    if (node.snapshot.ready === false) result.push(add('error', 'Нода не готова принимать подключения'));
    const idle = value(host.cpu_idle_percent);
    if (idle !== null && idle < 10) result.push(add('warning', `CPU свободно ${percent(idle)} (<10%)`));
    const memory = ratio(used(host.memory_total_bytes, host.memory_available_bytes), host.memory_total_bytes);
    if (memory !== null && memory >= 90) result.push(add('warning', `Память занята на ${percent(memory)} (≥90%)`));
    const disk = object(host.disk), diskUsed = ratio(used(disk.total_bytes, disk.available_bytes), disk.total_bytes);
    if (diskUsed !== null && diskUsed >= 90) result.push(add('warning', `Файловая система / занята на ${percent(diskUsed)} (≥90%)`));
    if (dcFresh(node, now) && dcRows(node).some(row => value(row.available_pct) !== null && Number(row.available_pct) < 100)) result.push(add('warning', 'Часть соединений с DC недоступна'));
    return result;
  });
}

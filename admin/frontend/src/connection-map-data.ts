export type MapLocation = { latitude: number; longitude: number; country: string; city: string };
export type MapClient = { ip: string; connections: number; users: number; location: MapLocation | null };
export type MapSnapshot = { available: true; generated_at: string; database: { status: string; build_epoch: number | null }; ips: MapClient[]; endpoints: { ip: string; location: MapLocation | null }[]; total_ips: number; connections: number; active_users: number; cleanup_pending: number; truncated: boolean; located_ips: number; non_public_ips: number };
export type MapResponse = MapSnapshot | { available: false; reason: string };
export type MapCluster = { id: string; location: MapLocation; ips: MapClient[]; connections: number; family: number };
export const MAX_MAP_POINTS = 300;
const count = (v: unknown): v is number => typeof v === 'number' && Number.isSafeInteger(v) && v >= 0;
const obj = (v: unknown): Record<string, unknown> => v !== null && typeof v === 'object' && !Array.isArray(v) ? v as Record<string, unknown> : {};
export function validLocation(value: unknown): value is MapLocation {
  const v = obj(value);
  return typeof v.latitude === 'number' && Number.isFinite(v.latitude) && Math.abs(v.latitude) <= 90 && typeof v.longitude === 'number' && Number.isFinite(v.longitude) && Math.abs(v.longitude) <= 180 && typeof v.city === 'string' && typeof v.country === 'string';
}
/** Fail closed before remote coordinates or counters reach the renderer. */
export function parseMap(value: unknown): MapResponse {
  const v = obj(value), database = obj(v.database);
  if (v.available === false && typeof v.reason === 'string') return { available: false, reason: v.reason };
  if (v.available !== true || typeof v.generated_at !== 'string' || !Number.isFinite(Date.parse(v.generated_at)) || typeof database.status !== 'string' || !(database.build_epoch === null || (count(database.build_epoch) && database.build_epoch <= 8640000000000)) || !Array.isArray(v.ips) || v.ips.length > 10000 || !Array.isArray(v.endpoints) || v.endpoints.length > 2 || typeof v.truncated !== 'boolean') throw new Error('Неверный ответ карты подключений');
  for (const key of ['total_ips', 'connections', 'active_users', 'cleanup_pending', 'located_ips', 'non_public_ips']) if (!count(v[key])) throw new Error('Некорректные счётчики подключений');
  if ((v.located_ips as number) > v.ips.length || (v.non_public_ips as number) > v.ips.length || (v.total_ips as number) < v.ips.length) throw new Error('Несогласованные счётчики карты');
  const seen = new Set<string>();
  for (const item of v.ips) {
    const row = obj(item);
    if (typeof row.ip !== 'string' || row.ip.length > 45 || seen.has(row.ip) || !count(row.connections) || !count(row.users) || row.connections < 1 || row.users < 1 || row.users > row.connections || !(row.location === null || validLocation(row.location))) throw new Error('Некорректная точка подключения');
    seen.add(row.ip);
  }
  for (const item of v.endpoints) { const row = obj(item); if (typeof row.ip !== 'string' || row.ip.length > 45 || !(row.location === null || validLocation(row.location))) throw new Error('Некорректные координаты ноды'); }
  return value as MapSnapshot;
}
/** Two-degree geographic bins bound SVG work while preserving every admitted IP in the drill-down. */
export function clusterClients(ips: MapClient[]): MapCluster[] {
  const bins = new Map<string, MapCluster>();
  for (const ip of ips) {
    if (!ip.location) continue;
    const family = ip.ip.includes(':') ? 6 : 4;
    const id = `${Math.floor((ip.location.longitude + 180) / 2)}:${Math.floor((ip.location.latitude + 90) / 2)}:${family}`;
    const cluster = bins.get(id);
    if (cluster) { cluster.ips.push(ip); cluster.connections += ip.connections; }
    else bins.set(id, { id, location: ip.location, ips: [ip], connections: ip.connections, family });
  }
  return [...bins.values()].sort((a, b) => b.connections - a.connections || a.id.localeCompare(b.id));
}
export function place(location: MapLocation) { return [location.city, location.country].filter(Boolean).join(', ') || 'Координаты без названия'; }

import { errorLabel } from './labels';
export type Json = null | boolean | number | string | Json[] | { [key: string]: Json };
export type User = { id: string; username: string; role: 'owner' | 'operator' | 'viewer' };
export type Node = { id: string; name: string; api_url: string; document: Record<string, Json>; snapshot: Record<string, Json>; collected_at: string | null; enabled: boolean };
export type Operation = { id: string; state: string; results: Json[]; created_at: string; updated_at: string };
export type Group = { id: string; name: string; active_node: string; automatic: boolean; status: string; document: Record<string, Json>; observed: Json };
let csrf = '';
export function setCsrf(value: string) { csrf = value; }
/** Reports Fleet routing failures without exposing upstream HTML or response secrets. */
export async function api<T>(path: string, method = 'GET', body?: Json, signal?: AbortSignal): Promise<T> {
  const response = await fetch(`/api/${path}`, { method, signal, credentials: 'same-origin', headers: { ...(body !== undefined ? { 'Content-Type': 'application/json' } : {}), ...(method !== 'GET' ? { 'X-CSRF-Token': csrf } : {}) }, body: body === undefined ? undefined : JSON.stringify(body) });
  if (response.status === 401 && path !== 'login') window.dispatchEvent(new Event('telemt-session-expired'));
  const target = `/api/${path.split('?')[0]}`;
  const mediaType = response.headers.get('Content-Type')?.split(';')[0].trim().toLowerCase();
  if (mediaType !== 'application/json' && !/^application\/[a-z0-9!#$&^_.+-]+\+json$/.test(mediaType ?? '')) {
    throw new Error(`API Fleet вернул ответ не в формате JSON (HTTP ${response.status}) на ${target}. Проверьте обратный прокси Fleet и страницы ошибок CDN.`);
  }
  let parsed: Json;
  try { parsed = await response.json(); } catch { throw new Error(`API Fleet вернул некорректный JSON (HTTP ${response.status}) на ${target}.`); }
  const value = object(parsed);
  if (typeof value.ok !== 'boolean') throw new Error(`API Fleet вернул неверную структуру ответа (HTTP ${response.status}) на ${target}.`);
  if (!response.ok || !value.ok) {
    const code = object(value.error).code;
    throw new Error(typeof code === 'string' ? errorLabel(code) : `HTTP ${response.status}`);
  }
  return value.data as T;
}
export function object(value: Json | undefined): Record<string, Json> { return value && typeof value === 'object' && !Array.isArray(value) ? value : {}; }
export function list(value: Json | undefined): Json[] { return Array.isArray(value) ? value : []; }
export function text(value: Json | undefined): string { return value === null || value === undefined ? 'Нет данных' : typeof value === 'object' ? JSON.stringify(value) : String(value); }
export function rate(value: Json | undefined): string { if (typeof value !== 'number') return 'Нет данных'; return `${(value / 1e6).toFixed(2)} Мбит/с`; }
export function bytes(value: Json | undefined): string { if (typeof value !== 'number') return 'Нет данных'; return `${(value / 1048576).toFixed(2)} МиБ`; }
export function isStale(node: Node, now = Date.now()) { return !node.collected_at || now - Date.parse(node.collected_at) > 15000; }
export async function readNode(id: string, path: string) {
  const data = await api<{ status: number; response: { ok: boolean; data: Json; revision: string; error?: { code: string } } }>(`nodes/${id}/read?path=${encodeURIComponent(path)}`);
  if (data.status >= 400 || !data.response.ok) throw new Error(data.response.error?.code ? errorLabel(data.response.error.code) : `Нода вернула HTTP ${data.status}`);
  return data.response;
}
export async function mutateNodes(ids: string[], method: string, path: string, body: Json) {
  const revisions: Record<string, string> = {};
  await Promise.all(ids.map(async id => { revisions[id] = (await readNode(id, '/v1/config')).revision; }));
  return api<{ id: string; state: string }>('operations', 'POST', { node_ids: ids, method, path, body, revisions });
}
export function generateSecret() { return [...crypto.getRandomValues(new Uint8Array(16))].map(byte => byte.toString(16).padStart(2, '0')).join(''); }

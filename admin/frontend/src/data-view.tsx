import { useState } from 'react';
import type { Json } from './api';
import { labelOf, stateLabel, number, date } from './labels';

/** Nested payloads mount only on demand; large arrays are expanded in bounded pages. */
function DataSection({ title, value, path, depth }: { title: string; value: Json; path: string; depth: number }) {
  const [open, setOpen] = useState(false);
  return <details className="data-section" open={open} onToggle={event => setOpen(event.currentTarget.open)}><summary>{title}<span className="count">{Array.isArray(value) ? `${value.length} записей` : `${Object.keys(value ?? {}).length} полей`}</span></summary>{open && <DataView value={value} path={path} depth={depth} />}</details>;
}
function DataList({ value, path, depth }: { value: Json[]; path: string; depth: number }) {
  const [limit, setLimit] = useState(25);
  return <div className="data-list">{value.slice(0, limit).map((item, i) => typeof item === 'object' && item !== null ? <DataSection key={i} title={`Запись ${i + 1}`} value={item} path={`${path}.${i}`} depth={depth + 1} /> : <div className="data-list-value" key={i}><span className="muted">{i + 1}</span><DataView value={item} path={`${path}.${i}`} depth={depth + 1} /></div>)}{value.length > limit && <button className="quiet" onClick={() => setLimit(limit + 25)}>Показать ещё · осталось {value.length - limit}</button>}</div>;
}
/** Unknown fields remain accessible without expanding a tree of cards on page load. */
export function DataView({ value, path = '', depth = 0 }: { value: Json | undefined; path?: string; depth?: number }) {
  if (value === null || value === undefined) return <span className="muted">Нет данных</span>;
  if (typeof value === 'boolean') return <span className={`badge ${value ? 'good' : ''}`}>{value ? 'Да' : 'Нет'}</span>;
  if (typeof value === 'number' && path.endsWith('_epoch_secs')) { const timestamp = new Date(value * 1000); return <span>{value === 0 ? 'Не зарегистрировано' : value > 0 && Number.isFinite(timestamp.getTime()) ? date(timestamp.toISOString()) : 'Некорректная дата'}</span>; }
  if (typeof value === 'number') return <span className="metric-value">{number(value)}</span>;
  if (typeof value === 'string') return <span className="fact-value">{/(?:_at|^at)$/.test(path.split('.').at(-1) ?? '') ? date(value) : ['state','status','role','mode','type','outcome','reason','action','lifecycle','management','stage'].includes(path.split('.').at(-1) ?? '') ? stateLabel(value || '—') : (value || '—')}</span>;
  if (depth >= 16) return <span className="muted">Превышена глубина вложенности данных</span>;
  if (Array.isArray(value)) return value.length ? <DataList value={value} path={path} depth={depth} /> : <span className="muted">Нет записей</span>;
  const entries = Object.entries(value);
  if (!entries.length) return <span className="muted">Нет показателей</span>;
  return <div className="data-sections">{entries.map(([key, entry]) => typeof entry === 'object' && entry !== null ? <DataSection key={key} title={labelOf(key)} value={entry} path={path ? `${path}.${key}` : key} depth={depth + 1} /> : <div className="fact" key={key}><span title={path ? `${path}.${key}` : key}>{labelOf(key)}</span><DataView value={entry} path={key} depth={depth + 1} /></div>)}</div>;
}

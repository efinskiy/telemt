import type { Json } from './api';
import { labelOf, stateLabel, number, date } from './labels';

/** Expands arbitrary diagnostic data without hiding unknown fields or leaking it into HTML. */
export function DataView({ value, path = '', depth = 0 }: { value: Json | undefined; path?: string; depth?: number }) {
  if (value === null || value === undefined) return <span className="muted">Нет данных</span>;
  if (typeof value === 'boolean') return <span className={`badge ${value ? 'good' : ''}`}>{value ? 'Да' : 'Нет'}</span>;
  if (typeof value === 'number' && path.endsWith('_epoch_secs')) { const timestamp = new Date(value * 1000); return <span>{value === 0 ? 'Не зарегистрировано' : value > 0 && Number.isFinite(timestamp.getTime()) ? date(timestamp.toISOString()) : 'Некорректная дата'}</span>; }
  if (typeof value === 'number') return <span className="metric-value">{number(value)}</span>;
  if (typeof value === 'string') return <span className="fact-value">{/(?:_at|^at)$/.test(path.split('.').at(-1) ?? '') ? date(value) : ['state','status','role','mode','type','outcome','reason','action','lifecycle','management'].includes(path.split('.').at(-1) ?? '') ? stateLabel(value || '—') : (value || '—')}</span>;
  if (depth >= 16) return <span className="muted">Превышена глубина вложенности данных</span>;
  if (Array.isArray(value)) {
    if (!value.length) return <span className="muted">Нет записей</span>;
    return <div className="data-list">{value.map((item, i) => <div className="data-item" key={i}>{typeof item === 'object' && item !== null && <h4>Запись {i + 1}</h4>}<DataView value={item} path={`${path}.${i}`} depth={depth + 1} /></div>)}</div>;
  }
  const entries = Object.entries(value);
  if (!entries.length) return <span className="muted">Нет показателей</span>;
  return <div className="data-sections">{entries.map(([key, entry]) => typeof entry === 'object' && entry !== null ? <details open={depth < 2} className="data-section" key={key}><summary>{labelOf(key)}{Array.isArray(entry) && <span className="count">{entry.length}</span>}</summary><DataView value={entry} path={path ? `${path}.${key}` : key} depth={depth + 1} /></details> : <div className="fact" key={key}><span title={path ? `${path}.${key}` : key}>{labelOf(key)}</span><DataView value={entry} path={key} depth={depth + 1} /></div>)}</div>;
}

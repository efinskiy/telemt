import { useState, useEffect, useRef } from 'react';
import type { Json } from './api';
import { text, object } from './api';
import { labelOf, stateLabel } from './labels';
import { DataView } from './data-view';

export function Badge({ value }: { value: string }) { return <span className={`badge ${['healthy', 'succeeded', 'reachable', 'verified'].includes(value) ? 'good' : ['failed', 'unreachable', 'unknown', 'external_dns_conflict'].includes(value) ? 'bad' : ''}`}>{stateLabel(value)}</span>; }
export function Empty({ children }: { children: React.ReactNode }) { return <div className="empty">{children}</div>; }
/** Structured presentation shared by diagnostics, operation results and one-time credentials. */
export function JsonView({ value }: { value: Json }) { return <DataView value={value} />; }
/** Table cells stay compact; full payloads remain available in a separate inspector. */
export function Table({ rows, columns, actions, cells = {} }: { rows: Json[]; columns: string[]; actions?: (row: Record<string, Json>) => React.ReactNode; cells?: Record<string, (row: Record<string, Json>) => React.ReactNode> }) {
  const [detail, setDetail] = useState<{ title: string; value: Json } | null>(null), [page, setPage] = useState(0);
  if (!rows.length) return <Empty>Нет записей</Empty>;
  const pages = Math.ceil(rows.length / 25), current = Math.min(page, pages - 1);
  const cell = (data: Record<string, Json>, column: string) => {
    if (cells[column]) return cells[column](data);
    const value = data[column];
    if (value !== null && value !== undefined && (typeof value === 'object' || typeof value === 'string' && value.length > 160)) {
      const summary = Array.isArray(value) ? `${value.length} записей` : typeof value === 'object' ? `${Object.keys(value).length} полей` : `${value.length} символов`;
      return <button className="quiet data-inspect" aria-label={`Открыть: ${labelOf(column)}`} onClick={() => setDetail({ title: labelOf(column), value })}>Подробнее <small>{summary}</small></button>;
    }
    return column === 'state' || column === 'status' ? <Badge value={text(value)} /> : <DataView value={value} path={column} />;
  };
  return <><div className="table-wrap"><table><thead><tr>{columns.map(column => <th key={column}>{labelOf(column)}</th>)}{actions && <th>Действия</th>}</tr></thead><tbody>{rows.slice(current * 25, (current + 1) * 25).map((row, index) => { const data = object(row); return <tr key={text(data.id ?? data.username ?? index)}>{columns.map(column => <td key={column}>{cell(data, column)}</td>)}{actions && <td className="actions">{actions(data)}</td>}</tr>; })}</tbody></table></div>{pages > 1 && <div className="table-pagination"><span>{current * 25 + 1}–{Math.min((current + 1) * 25, rows.length)} из {rows.length}</span><button className="quiet" disabled={current === 0} onClick={() => setPage(current - 1)}>Назад</button><button className="quiet" disabled={current === pages - 1} onClick={() => setPage(current + 1)}>Далее</button></div>}{detail && <Dialog title={detail.title} close={() => setDetail(null)}><DataView value={detail.value} path={detail.title} /></Dialog>}</>;
}
export function Dialog({ title, children, close }: { title: string; children: React.ReactNode; close: () => void }) {
  const ref = useRef<HTMLElement>(null);
  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    ref.current?.querySelector<HTMLElement>('button,input,select,textarea,a[href]')?.focus();
    const key = (event: KeyboardEvent) => {
      // Nested inspectors must not dismiss or move focus into their parent dialog.
      const dialogs = document.querySelectorAll('[role="dialog"]');
      if (dialogs.item(dialogs.length - 1) !== ref.current) return;
      if (event.key === 'Escape') { event.preventDefault(); close(); }
      if (event.key === 'Tab') {
        const items = [...(ref.current?.querySelectorAll<HTMLElement>('button:not(:disabled),input:not(:disabled),select:not(:disabled),textarea:not(:disabled),a[href],summary') ?? [])].filter(item => item.getClientRects().length > 0);
        const first = items[0], last = items.at(-1);
        if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
        else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
      }
    };
    document.addEventListener('keydown', key); return () => { document.removeEventListener('keydown', key); previous?.focus(); };
  }, []);
  return <div className="dialog-backdrop" onClick={close}><section ref={ref} className="dialog" role="dialog" aria-modal="true" aria-label={title} onClick={event => event.stopPropagation()}><div className="section-title"><h2>{title}</h2><button className="quiet" onClick={close} aria-label="Закрыть окно">Закрыть</button></div>{children}</section></div>;
}
export type Field = { key: string; label: string; kind?: 'number' | 'password' | 'checkbox' | 'select'; options?: string[]; required?: boolean; min?: number; max?: number; description?: string };
export function FieldsForm({ fields, initial, onSave, disabled = false, label = 'Сохранить' }: { fields: Field[]; initial: Record<string, Json>; onSave: (value: Record<string, Json>) => Promise<void>; disabled?: boolean; label?: string }) {
  const [value, setValue] = useState(initial), [error, setError] = useState(''), [busy, setBusy] = useState(false);
  return <form onSubmit={async event => { event.preventDefault(); setBusy(true); setError(''); try { await onSave(value); } catch (error) { setError(String(error)); } finally { setBusy(false); } }}><div className="form-grid">{fields.map(field => <label key={field.key} className="label">{field.label}{field.kind === 'checkbox' ? <input aria-label={field.label} type="checkbox" checked={value[field.key] === true} disabled={disabled || busy} onChange={event => setValue({ ...value, [field.key]: event.target.checked })} /> : field.kind === 'select' ? <select aria-label={field.label} value={String(value[field.key] ?? '')} disabled={disabled || busy} onChange={event => setValue({ ...value, [field.key]: event.target.value })}>{field.options?.map(option => <option key={option} value={option}>{stateLabel(option)}</option>)}</select> : <input aria-label={field.label} type={field.kind ?? 'text'} value={String(value[field.key] ?? '')} min={field.min} max={field.max} required={field.required} disabled={disabled || busy} autoComplete={field.kind === 'password' ? 'new-password' : undefined} onChange={event => setValue({ ...value, [field.key]: field.kind === 'number' ? event.target.value === '' ? null : Number(event.target.value) : event.target.value })} />}{field.description && <small className="field-help">{field.description}</small>}</label>)}</div>{error && <p role="alert" className="error">{error}</p>}<button disabled={disabled || busy}>{busy ? 'Выполняется…' : label}</button></form>;
}

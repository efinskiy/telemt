import { useId } from 'react';
import { Icon } from './interface';
import { number } from './labels';
export type Sample = { at: number; values: (number | null)[] };
/** Missing measurements and collection gaps remain visible instead of becoming zeroes. */
export function segments(samples: Sample[], series: number, gap: number) {
  const result: Sample[][] = []; let current: Sample[] = [];
  for (const sample of samples) {
    const value = sample.values[series];
    if (value === null || value === undefined || !Number.isFinite(value)) { if (current.length) result.push(current); current = []; continue; }
    if (current.length && sample.at - current.at(-1)!.at > gap) { result.push(current); current = []; }
    current.push(sample);
  }
  if (current.length) result.push(current);
  return result;
}
const colors = ['var(--chart-one)', 'var(--chart-two)', 'var(--chart-three)'];
export function Chart({ title, samples, series, unit = '', gap = 90000 }: { title: string; samples: Sample[]; series: string[]; unit?: string; gap?: number }) {
  const id = useId(); const ordered = samples.filter(row => Number.isFinite(row.at)).slice().sort((a, b) => a.at - b.at);
  const values = ordered.flatMap(row => row.values.slice(0, series.length).filter((value): value is number => value !== null && Number.isFinite(value)));
  const max = Math.max(1, ...values); const start = ordered[0]?.at ?? 0; const end = ordered.at(-1)?.at ?? start;
  const x = (at: number) => 66 + (at - start) / Math.max(1, end - start) * 650;
  const y = (value: number) => 180 - value / max * 150;
  return <section className="chart-panel"><h2>{title}</h2><div className="chart-legend">{series.map((name, i) => <span key={name}><i style={{ background: colors[i % colors.length] }} />{name}</span>)}<span>{unit}</span></div>{!values.length ? <div className="empty">Нет измерений за выбранный период</div> : <svg className="history-chart" viewBox="0 0 750 220" role="img" aria-labelledby={id}><title id={id}>{`${title}. ${series.join(', ')}. Пробелы означают отсутствие измерений.`}</title>{[0, .5, 1].map(fraction => <g key={fraction}><line x1="66" x2="716" y1={y(max * fraction)} y2={y(max * fraction)} className="grid-line" /><text x="57" y={y(max * fraction) + 4} textAnchor="end">{number(max * fraction, 1)}</text></g>)}{series.map((name, i) => <g key={name}>{segments(ordered, i, gap).map((part, j) => <g key={j}><polyline fill="none" stroke={colors[i % colors.length]} strokeWidth="2.5" points={part.map(row => `${x(row.at)},${y(row.values[i]!)}`).join(' ')} />{part.map(row => <circle key={row.at} cx={x(row.at)} cy={y(row.values[i]!)} r={part.length === 1 ? 3 : 2} fill={colors[i % colors.length]}><title>{`${new Date(row.at).toLocaleString('ru-RU')} · ${name}: ${number(row.values[i], 2)} ${unit}`}</title></circle>)}</g>)}</g>)}<text x="66" y="210">{new Date(start).toLocaleTimeString('ru-RU')}</text><text x="716" y="210" textAnchor="end">{new Date(end).toLocaleTimeString('ru-RU')}</text></svg>}<p className="muted">Наведение на точку показывает время и значение. Пропуски данных не соединяются.</p></section>;
}
export function Metric({ label, value, hint }: { label: string; value: React.ReactNode; hint?: string }) { return <div className="card"><span className="metric-label"><Icon name={/пользоват|Пользоват/.test(label) ? 'users' : /сервер|Сервер/.test(label) ? 'server' : /IP/.test(label) ? 'globe' : 'activity'} />{label}</span><strong>{value}</strong>{hint && <small>{hint}</small>}</div>; }
export function Meter({ label, value, maximum }: { label: string; value: number | null; maximum: number | null }) {
  const ratio = value !== null && maximum !== null && maximum > 0 ? Math.min(100, value / maximum * 100) : null;
  return <div className="meter"><div><span>{label}</span><strong>{ratio === null ? 'Нет данных' : `${number(ratio, 1)}%`}</strong></div><progress aria-label={label} value={ratio ?? 0} max={100} /></div>;
}

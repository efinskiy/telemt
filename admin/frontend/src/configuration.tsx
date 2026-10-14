import { useEffect, useState } from 'react';
import { type Json, object, readNode } from './api';
import { DataView } from './data-view';
import { labelOf, stateLabel } from './labels';
import { templates, optionals, choices, helpFor } from './config-schema';

/** Lists replace as a whole; retained object members survive edits inside a list. */
export function ConfigField({ value, path, onChange, disabled = false }: { value: Json; path: string; onChange: (value: Json) => void; disabled?: boolean }) {
  const key = path.split('.').at(-1)!; const [addition, setAddition] = useState('');
  const title = path === 'censorship.tls_fetch.profiles' ? 'Профили TLS' : /^\d+$/.test(key) ? `Значение ${Number(key) + 1}` : labelOf(key), hint = helpFor(path);
  if (key === 'carriers' || key === 'show' || key === 'synlimit') {
    const union = key === 'carriers' ? ['off', 'list'] : key === 'show' ? ['all', 'list'] : ['off', 'iptables', 'nftables', 'pf'];
    const current = Array.isArray(value) ? 'list' : value === false ? 'off' : value === '*' ? 'all' : String(value);
    return <div className="config-block"><label className="label">{title}<select aria-label={title} value={current} disabled={disabled} onChange={event => onChange(event.target.value === 'off' ? false : event.target.value === 'all' ? '*' : event.target.value === 'list' ? key === 'carriers' ? ['https'] : [] : event.target.value)}>{union.map(item => <option key={item} value={item}>{({off:'Выключено',list:'Список',all:'Все пользователи'} as Record<string,string>)[item] ?? item}</option>)}</select></label><p className="field-help">{hint}</p>{Array.isArray(value) && <ConfigField value={value} path={`${path}.items`} disabled={disabled} onChange={onChange} />}</div>;
  }
  if (Array.isArray(value)) {
    const fixed = key === 'carrier_negotiation_deadlines_secs'; const template = key === 'profiles' && !path.startsWith('web.') ? 'modern_chrome_like' : templates[key] ?? (fixed ? 0 : '');
    return <div className="config-block"><h3>{title}</h3><p className="field-help">{hint} {fixed ? 'Четыре позиции, порядок фиксирован.' : 'Порядок элементов сохраняется.'}</p>{value.map((entry, index) => <div className="config-item" key={index}><div className="section-title"><strong>Элемент {index + 1}</strong>{!fixed && <div className="actions"><button type="button" className="quiet" disabled={disabled || index === 0} onClick={() => { const next = [...value]; [next[index-1], next[index]] = [next[index], next[index-1]]; onChange(next); }}>Выше</button><button type="button" className="danger" disabled={disabled} onClick={() => onChange(value.filter((_, i) => i !== index))}>Удалить элемент</button></div>}</div><ConfigField path={`${path}.${index}`} value={entry} disabled={disabled} onChange={changed => onChange(value.map((item, i) => i === index ? changed : item))} /></div>)}{!fixed && <button type="button" className="quiet" disabled={disabled || value.length >= 1000} onClick={() => onChange([...value, structuredClone(template)])}>Добавить элемент · {title}</button>}</div>;
  }
  if (value !== null && typeof value === 'object') {
    const parent = path.split('.').at(-2) ?? ''; const rawCandidates = optionals[key] ?? optionals[parent] ?? {};
    const variants: Record<string, string[]> = { direct:['bind_addresses','bindtodevice'], socks4:['user_id'], socks5:['username','password'], shadowsocks:[] };
    const candidates = Object.fromEntries(Object.entries(rawCandidates).filter(([field]) => parent !== 'upstreams' || ['interface','ipv4','ipv6','prefer'].includes(field) || (variants[String(value.type)] ?? []).includes(field)));
    const dynamic = key === 'dc_overrides' || key === 'exclusive_mask';
    return <div className="config-object"><div className="form-grid">{Object.entries(value).map(([name, entry]) => <div key={name} className={typeof entry === 'object' && entry !== null ? 'config-object' : ''}>{entry === null && name in candidates ? <button type="button" className="quiet" disabled={disabled} onClick={() => onChange({ ...value, [name]: structuredClone(candidates[name]) })}>Задать · {labelOf(name)}</button> : <ConfigField path={`${path}.${name}`} value={entry} disabled={disabled} onChange={changed => {
      let next: Record<string, Json> = { ...value, [name]: changed };
      if (name === 'type' && parent === 'upstreams') {
        const common = Object.fromEntries(Object.entries(value).filter(([field]) => ['weight','enabled','scopes','ipv4','ipv6','prefer','interface'].includes(field)));
        next = { ...common, type: changed, ...(changed === 'socks4' || changed === 'socks5' ? { address: '' } : changed === 'shadowsocks' ? { url: '' } : {}) };
      }
      if (name === 'mode' && key === 'decoy') next = changed === 'http_upstream' ? { mode: changed, upstream: '', resolve: 'never' } : { mode: changed, directory: '', index: 'index.html' };
      onChange(next);
    }} />}{name in candidates && templates[parent] && <button type="button" className="quiet" disabled={disabled} onClick={() => onChange(Object.fromEntries(Object.entries(value).filter(([key]) => key !== name)))}>Наследовать · {labelOf(name)}</button>}</div>)}</div>{Object.keys(candidates).some(name => !(name in value)) && <div className="actions"><label className="label">Дополнительный параметр<select aria-label={`Дополнительный параметр ${path}`} value={addition} disabled={disabled} onChange={event => setAddition(event.target.value)}><option value="">Выберите параметр</option>{Object.keys(candidates).filter(name => !(name in value)).map(name => <option key={name} value={name}>{labelOf(name)}</option>)}</select></label><button type="button" className="quiet" disabled={disabled || !addition} onClick={() => { onChange({ ...value, [addition]: structuredClone(candidates[addition]) }); setAddition(''); }}>Добавить параметр</button></div>}{dynamic && <div className="actions"><label className="label">{key === 'dc_overrides' ? 'Номер DC' : 'Домен'}<input value={addition} disabled={disabled} onChange={event => setAddition(event.target.value)} /></label><button type="button" className="quiet" disabled={disabled || !addition.trim() || addition.trim() in value || ['__proto__','constructor','prototype'].includes(addition.trim())} onClick={() => { onChange({ ...value, [addition.trim()]: key === 'dc_overrides' ? [''] : '' }); setAddition(''); }}>Добавить запись</button><p className="field-help">Словарь задаётся в общем профиле и применяется ко всем его нодам.</p></div>}</div>;
  }
  const options = path.startsWith('web.carriers.items.') ? ['https','https-lanes','websocket','websocket-lanes'] : path.startsWith('censorship.tls_fetch.profiles.') ? ['modern_chrome_like','modern_firefox_like','compat_tls12','legacy_minimal'] : choices[key]; const numeric = typeof value === 'number';
  return <label className="label config-scalar">{title}<code>{path}</code>{typeof value === 'boolean' ? <input aria-label={title} type="checkbox" checked={value} disabled={disabled} onChange={event => onChange(event.target.checked)} /> : options ? <select aria-label={title} value={String(value ?? '')} disabled={disabled} onChange={event => onChange(event.target.value)}>{!options.includes(String(value)) && <option value={String(value ?? '')}>{String(value ?? 'Не задано')}</option>}{options.map(option => <option key={option} value={option}>{stateLabel(option)}</option>)}</select> : numeric ? <NumberField title={title} value={value as number} disabled={disabled} onChange={onChange} /> : <input aria-label={title} type={/password|secret|token|\.url$/.test(path) ? 'password' : 'text'} value={value === null ? '' : String(value)} disabled={disabled} onChange={event => onChange(event.target.value)} />}<small className="field-help">{hint}{value === null && ' Не задано; используется значение по умолчанию профиля.'}</small></label>;
}
function NumberField({ title, value, disabled, onChange }: { title: string; value: number; disabled: boolean; onChange: (value: Json) => void }) {
  const [raw, setRaw] = useState(String(value));
  useEffect(() => { setRaw(String(value)); }, [value]);
  return <input aria-label={title} type="number" step="any" required value={raw} disabled={disabled} onChange={event => { setRaw(event.target.value); if (event.target.value !== '' && Number.isFinite(event.target.valueAsNumber)) onChange(event.target.valueAsNumber); }} />;
}
/** Inspect the generated node configuration without bypassing profile ownership. */
export function Configuration({ nodeId }: { nodeId: string }) {
  const [config, setConfig] = useState<Json | null>(null), [revision, setRevision] = useState(''), [error, setError] = useState(''), [reload, setReload] = useState(0);
  useEffect(() => { let active = true; setConfig(null); setError(''); readNode(nodeId, '/v1/config').then(response => { if (active) { setConfig(object(response.data).config ?? response.data); setRevision(response.revision); } }).catch(error => { if (active) setError(String(error)); }); return () => { active = false; }; }, [nodeId, reload]);
  return <section><div className="section-title"><h2>Настройки ноды</h2><button className="quiet" onClick={() => setReload(value => value + 1)}>Загрузить сохранённую конфигурацию</button></div><p>Конфигурация формируется общим профилем. Изменения публикуются в разделе «Общие профили»; подтверждённая версия каждой ноды отображается там же.</p><p className="revision">Ревизия: {revision || 'Загрузка…'}</p>{error && <p role="alert" className="error">{error}</p>}{config && <DataView value={config} />}</section>;
}

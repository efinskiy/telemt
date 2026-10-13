import { useEffect, useId, useMemo, useRef, useState } from 'react';
import { geoDistance, geoEquirectangular, geoGraticule10, geoOrthographic, geoPath, type GeoPermissibleObjects } from 'd3-geo';
import land from './world-land.json';
import { MAX_MAP_POINTS, place, type MapCluster, type MapSnapshot } from './connection-map-data';

const WIDTH = 960, HEIGHT = 480;
const normalizeLongitude = (value: number) => ((value + 180) % 360 + 360) % 360 - 180;
export function ConnectionGlobe({ snapshot, clusters, selected, select }: { snapshot: MapSnapshot | null; clusters: MapCluster[]; selected: string | null; select: (id: string | null) => void }) {
  const [mode, setMode] = useState<'globe' | 'flat'>('globe'), [center, setCenter] = useState<[number, number]>([45, 35]), [zoom, setZoom] = useState(1), [rotate, setRotate] = useState(false), [full, setFull] = useState(false), [hover, setHover] = useState<string | null>(null);
  const frame = useRef<HTMLDivElement>(null), drag = useRef<{ x: number; y: number; center: [number, number] } | null>(null);
  const lastSelection = useRef<string | null>(null);
  useEffect(() => {
    if (selected === lastSelection.current) return;
    lastSelection.current = selected;
    const point = clusters.find(row => row.id === selected);
    if (point) { setCenter([point.location.longitude, mode === 'globe' ? point.location.latitude : 0]); setRotate(false); }
  }, [selected, clusters, mode]);
  const id = useId().replace(/:/g, '');
  useEffect(() => {
    if (!rotate || mode !== 'globe' || window.matchMedia('(prefers-reduced-motion: reduce)').matches) return;
    const timer = setInterval(() => { if (!drag.current && !document.hidden) setCenter(([lon, lat]) => [normalizeLongitude(lon + .4), lat]); }, 80);
    return () => clearInterval(timer);
  }, [rotate, mode]);
  useEffect(() => { const changed = () => setFull(document.fullscreenElement === frame.current); document.addEventListener('fullscreenchange', changed); return () => document.removeEventListener('fullscreenchange', changed); }, []);
  const projection = useMemo(() => mode === 'globe'
    ? geoOrthographic().translate([WIDTH / 2, HEIGHT / 2]).scale(215 * zoom).rotate([-center[0], -center[1]]).clipAngle(90)
    : geoEquirectangular().translate([WIDTH / 2, HEIGHT / 2 + center[1] * zoom * 2]).scale(145 * zoom).rotate([-center[0], 0]).clipExtent([[0, 0], [WIDTH, HEIGHT]]), [mode, center, zoom]);
  const path = geoPath(projection);
  const landPath = useMemo(() => geoPath(projection)(land as GeoPermissibleObjects) ?? '', [projection]);
  const gridPath = useMemo(() => geoPath(projection)(geoGraticule10()) ?? '', [projection]);
  const visible = (lon: number, lat: number) => mode === 'flat' || geoDistance([lon, lat], center) < Math.PI / 2;
  const points = clusters.slice(0, MAX_MAP_POINTS);
  const active = clusters.find(row => row.id === (hover ?? selected));
  return <div className={`connection-globe${full ? ' is-fullscreen' : ''}`} ref={frame}>
    <div className="map-toolbar"><div role="group" aria-label="Вид карты"><button className={mode === 'globe' ? 'selected' : ''} aria-pressed={mode === 'globe'} onClick={() => { setMode('globe'); setCenter([45, 35]); setZoom(1); }}>Глобус</button><button className={mode === 'flat' ? 'selected' : ''} aria-pressed={mode === 'flat'} onClick={() => { setMode('flat'); setCenter([0, 0]); setZoom(1); }}>Карта</button></div><div className="map-tools"><button aria-label="Увеличить карту" onClick={() => setZoom(v => Math.min(4, v * 1.25))}>+</button><button aria-label="Уменьшить карту" onClick={() => setZoom(v => Math.max(.75, v / 1.25))}>−</button><button onClick={() => { setCenter(mode === 'globe' ? [45, 35] : [0, 0]); setZoom(1); setRotate(false); }}>Сброс</button>{mode === 'globe' && <button aria-pressed={rotate} onClick={() => setRotate(v => !v)}>Вращение {rotate ? 'вкл.' : 'выкл.'}</button>}<button onClick={() => { if (full) void document.exitFullscreen().catch(() => {}); else void frame.current?.requestFullscreen?.().catch(() => {}); }}>{full ? 'Свернуть' : 'На весь экран'}</button></div></div>
    <svg className="map-surface" viewBox={`0 0 ${WIDTH} ${HEIGHT}`} role="group" aria-label="Интерактивная карта активных подключений" tabIndex={0}
      onKeyDown={event => { const directions: Record<string, [number, number]> = { ArrowLeft: [-10, 0], ArrowRight: [10, 0], ArrowUp: [0, 10], ArrowDown: [0, -10] }; const delta = directions[event.key]; if (delta) { event.preventDefault(); setRotate(false); setCenter(([lon, lat]) => [normalizeLongitude(lon + delta[0]), Math.max(-80, Math.min(80, lat + delta[1]))]); } }}
      onWheel={event => { if (event.ctrlKey || event.metaKey) setZoom(v => Math.max(.75, Math.min(4, v * (event.deltaY < 0 ? 1.1 : .9)))); }}
      onPointerDown={event => { if ((event.target as Element).closest('[data-map-point]')) return; drag.current = { x: event.clientX, y: event.clientY, center }; event.currentTarget.setPointerCapture(event.pointerId); setRotate(false); }}
      onPointerMove={event => { if (!drag.current) return; const scale = WIDTH / Math.max(1, event.currentTarget.getBoundingClientRect().width); setCenter([normalizeLongitude(drag.current.center[0] - (event.clientX - drag.current.x) * scale / (3 * zoom)), Math.max(-80, Math.min(80, drag.current.center[1] + (event.clientY - drag.current.y) * scale / (3 * zoom)))]); }}
      onPointerUp={() => { drag.current = null; }} onPointerCancel={() => { drag.current = null; }} onLostPointerCapture={() => { drag.current = null; }}>
      <defs><radialGradient id={`${id}-ocean`} cx="35%" cy="25%"><stop offset="0%" stopColor="#173d53" /><stop offset="100%" stopColor="#081725" /></radialGradient><radialGradient id={`${id}-halo`}><stop offset="80%" stopColor="#52d8cf" stopOpacity="0" /><stop offset="94%" stopColor="#52d8cf" stopOpacity=".15" /><stop offset="100%" stopColor="#52d8cf" stopOpacity="0" /></radialGradient></defs>
      {mode === 'globe' && <circle cx={WIDTH / 2} cy={HEIGHT / 2} r={230 * zoom} fill={`url(#${id}-halo)`} />}
      <path d={path({ type: 'Sphere' }) ?? ''} fill={`url(#${id}-ocean)`} stroke="#315164" />
      <path d={gridPath} fill="none" stroke="#264658" strokeWidth=".6" />
      <path d={landPath} fill="#23495b" stroke="#518094" strokeWidth=".55" />
      {points.map(row => {
        const destination = snapshot?.endpoints.find(endpoint => (endpoint.ip.includes(':') ? 6 : 4) === row.family && endpoint.location)?.location;
        if (!destination) return null;
        const line = path({ type: 'LineString', coordinates: [[row.location.longitude, row.location.latitude], [destination.longitude, destination.latitude]] });
        return <path key={`route-${row.id}`} d={line ?? ''} className={`map-route${selected === row.id ? ' active' : ''}`} />;
      })}
      {points.map(row => { const { longitude, latitude } = row.location; if (!visible(longitude, latitude)) return null; const point = projection([longitude, latitude]); if (!point) return null;
        return <g key={row.id} data-map-point="client" role="button" tabIndex={0} aria-label={`${place(row.location)}: ${row.connections} соединений, ${row.ips.length} IP`} aria-pressed={selected === row.id} transform={`translate(${point[0]},${point[1]})`} onMouseEnter={() => setHover(row.id)} onMouseLeave={() => setHover(null)} onFocus={() => setHover(row.id)} onBlur={() => setHover(null)} onClick={() => select(selected === row.id ? null : row.id)} onKeyDown={event => { if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); select(selected === row.id ? null : row.id); } }}><circle className="map-hit" r="13" /><circle className={`map-client${selected === row.id ? ' active' : ''}`} r={Math.min(10, 3 + Math.log2(row.connections + 1))} /><title>{place(row.location)} · {row.connections} соединений · {row.ips.length} IP</title></g>;
      })}
      {snapshot?.endpoints.map(endpoint => { const loc = endpoint.location; if (!loc || !visible(loc.longitude, loc.latitude)) return null; const point = projection([loc.longitude, loc.latitude]); return point && <g key={endpoint.ip} transform={`translate(${point[0]},${point[1]})`}><circle r="12" className="map-node-halo" /><path d="M0,-7 L7,0 L0,7 L-7,0Z" className="map-node" /><title>Нода · {endpoint.ip} · {place(loc)}</title></g>; })}
    </svg>
    <div className="map-caption"><span><i className="client-dot" />Клиенты <i className="node-dot" />Нода</span><span>{active ? `${place(active.location)} · ${active.connections} соединений · ${active.ips.length} IP` : 'Перетаскивайте карту · стрелки для поворота · выберите точку'}</span></div>
  </div>;
}

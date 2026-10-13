import { describe, expect, it } from 'vitest';
import { geoArea } from 'd3-geo';
import type { GeoPermissibleObjects } from 'd3-geo';
import land from './world-land.json';
import { clusterClients, parseMap, validLocation, type MapSnapshot } from './connection-map-data';
const location = { latitude: 55.75, longitude: 37.62, country: 'Россия', city: 'Москва' };
const fixture = (): MapSnapshot => ({ available: true, generated_at: new Date().toISOString(), database: { status: 'ready', build_epoch: 1727740800 }, ips: [{ ip: '1.1.1.1', connections: 2, users: 1, location }], endpoints: [], connections: 2, active_users: 1, total_ips: 1, cleanup_pending: 0, truncated: false, located_ips: 1, non_public_ips: 0 });
describe('connection map boundaries', () => {
  it('rejects invalid locations, dates, duplicate addresses and excessive snapshots', () => {
    expect(validLocation({ ...location, latitude: 91 })).toBe(false);
    expect(validLocation({ ...location, longitude: NaN })).toBe(false);
    expect(() => parseMap([])).toThrow();
    const invalidDate = fixture(); invalidDate.database.build_epoch = 1e15; expect(() => parseMap(invalidDate)).toThrow();
    const wrongCounts = fixture(); wrongCounts.located_ips = 2; expect(() => parseMap(wrongCounts)).toThrow();
    const duplicate = fixture(); duplicate.ips.push(duplicate.ips[0]); duplicate.total_ips = 2; expect(() => parseMap(duplicate)).toThrow();
    const oversized = fixture(); oversized.ips = Array(10001).fill(oversized.ips[0]); expect(() => parseMap(oversized)).toThrow();
  });
  it('preserves unavailable versus empty and missing coordinates', () => {
    expect(parseMap({ available: false, reason: 'node_update_required' }).available).toBe(false);
    const empty = fixture(); empty.ips = []; empty.total_ips = 0; empty.connections = 0; empty.active_users = 0; empty.located_ips = 0;
    expect(parseMap(empty)).toEqual(empty);
    const unknown = fixture(); unknown.ips[0].location = null; unknown.located_ips = 0;
    expect(clusterClients((parseMap(unknown) as MapSnapshot).ips)).toEqual([]);
  });
  it('groups connections without summing unique accounts and separates IP families', () => {
    const ip = fixture().ips[0];
    const clusters = clusterClients([ip, { ...ip, ip: '8.8.8.8', connections: 5 }, { ...ip, ip: '2606:4700::1' }, { ...ip, ip: '10.0.0.1', location: null }]);
    expect(clusters).toHaveLength(2); expect(clusters[0].connections).toBe(7); expect(clusters[0].ips).toHaveLength(2); expect(clusters[1].family).toBe(6);
  });
  it('preserves totals across a 10000-IP input without expanding SVG points', () => {
    const rows = Array.from({ length: 10000 }, (_, i) => ({ ...fixture().ips[0], ip: `10.${Math.floor(i / 256)}.${i % 256}.1`, location: { ...location, latitude: i % 160 - 80, longitude: i % 350 - 175 } }));
    const clusters = clusterClients(rows);
    expect(clusters.reduce((sum, row) => sum + row.ips.length, 0)).toBe(10000);
    expect(clusters.reduce((sum, row) => sum + row.connections, 0)).toBe(20000);
  });
  it('world polygons cover land rather than inverse hemispheres', () => {
    const area = geoArea(land as GeoPermissibleObjects);
    expect(area).toBeGreaterThan(2); expect(area).toBeLessThan(5);
  });
});

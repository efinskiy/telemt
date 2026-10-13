import { describe, expect, it } from 'vitest';
import { isStale, object, list, rate, type Node } from './api';
describe('monitoring boundaries', () => {
  it('never labels absent throughput as zero', () => { expect(rate(null)).toBe('Unavailable'); expect(rate(0)).toBe('0.00 Mbps'); });
  it('marks missing or expired samples stale', () => { const node = { collected_at: null } as Node; expect(isStale(node, 20000)).toBe(true); expect(isStale({ ...node, collected_at: new Date(1000).toISOString() }, 20000)).toBe(true); });
  it('handles disabled and unavailable node features', () => { expect(object(null)).toEqual({}); expect(list({ unavailable: true })).toEqual([]); });
});

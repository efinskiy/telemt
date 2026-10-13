import { afterEach, describe, expect, it, vi } from 'vitest';
import { api } from './api';

afterEach(() => vi.unstubAllGlobals());

describe('Fleet API response boundary', () => {
  const path = 'nodes/test-node/read?path=%2Fv1%2Fusers';

  it.each([200, 403, 502])('identifies Fleet HTML responses with HTTP %s without exposing their body', async status => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response('<!DOCTYPE html><p>private-response-detail</p>', { status, headers: { 'Content-Type': 'text/html; charset=utf-8' } })));
    const failure = await api(path).catch(error => error as Error);
    expect(failure).toBeInstanceOf(Error);
    expect(String(failure)).toContain(`API Fleet вернул ответ не в формате JSON (HTTP ${status})`);
    expect(String(failure)).toContain('/api/nodes/test-node/read');
    expect(String(failure)).toContain('обратный прокси');
    expect(String(failure)).not.toContain('private-response-detail');
    expect(String(failure)).not.toContain('SyntaxError');
  });

  it('identifies malformed JSON without leaking the response', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response('{private-response-detail', { headers: { 'Content-Type': 'application/json' } })));
    await expect(api(path)).rejects.toThrow('API Fleet вернул некорректный JSON (HTTP 200)');
  });

  it.each(['null', '[]', '{"data":[]}'])('rejects invalid API envelopes: %s', async body => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(body, { headers: { 'Content-Type': 'application/json' } })));
    await expect(api(path)).rejects.toThrow('API Fleet вернул неверную структуру ответа (HTTP 200)');
  });

  it.each(['application/json', 'text/html'])('expires the session on a 401 with %s', async contentType => {
    const dispatchEvent = vi.fn();
    vi.stubGlobal('window', { dispatchEvent });
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(contentType === 'text/html' ? '<!DOCTYPE html>' : '{"ok":false,"error":{"code":"session_expired"}}', { status: 401, headers: { 'Content-Type': contentType } })));
    await expect(api(path)).rejects.toThrow();
    expect(dispatchEvent).toHaveBeenCalledOnce();
    expect(dispatchEvent.mock.calls[0][0].type).toBe('telemt-session-expired');
  });

  it('preserves node errors returned as JSON by Fleet', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(Response.json({ ok: false, error: { code: 'upstream_operation_failed' } }, { status: 502 })));
    await expect(api(path)).rejects.toThrow('upstream_operation_failed');
  });

  it('preserves successful data and does not expire sessions on a rejected login', async () => {
    const dispatchEvent = vi.fn();
    vi.stubGlobal('window', { dispatchEvent });
    vi.stubGlobal('fetch', vi.fn().mockResolvedValueOnce(Response.json({ ok: true, data: [1, 2] })).mockResolvedValueOnce(Response.json({ ok: false, error: { code: 'invalid_credentials' } }, { status: 401 })));
    await expect(api(path)).resolves.toEqual([1, 2]);
    await expect(api('login', 'POST', {})).rejects.toThrow('invalid_credentials');
    expect(dispatchEvent).not.toHaveBeenCalled();
  });
});

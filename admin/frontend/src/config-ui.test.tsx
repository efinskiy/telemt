import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { configPatch, editable } from './config-schema';
import { Chart, segments } from './charts';
import { DataView } from './data-view';
import { ConfigField } from './configuration';
describe('configuration merge contract', () => {
  it('changes only touched object fields and excludes installation fields', () => {
    const original = { general:{ log_level:'normal', telemetry:{ user_enabled:false } }, server:{ port:443, listeners:[] }, network:{ipv4:true} };
    expect(configPatch(editable(original), { ...editable(original), general:{ log_level:'debug', telemetry:{ user_enabled:false } } })).toEqual({ general:{log_level:'debug'} });
    expect(editable(original)).not.toHaveProperty('network'); expect(editable(original).server).toEqual({listeners:[]});
  });
  it('retains opaque array properties while replacing the entire array', () => {
    const before = { upstreams:[{type:'socks5',address:'old:1080',password:'secret',future:{setting:3}}] };
    const after = { upstreams:[{...before.upstreams[0],address:'new:1080'}] };
    expect(configPatch(before, after)).toEqual(after);
    expect(configPatch(before,before)).toBeUndefined();
  });
  it('does not pretend omission deletes merged dictionary entries', () => { expect(configPatch({dc_overrides:{'1':['ip']}},{dc_overrides:{}})).toBeUndefined(); });
  it('uses separate templates and units for TLS and WEB profiles', () => {
    const tls = renderToStaticMarkup(<ConfigField value={['modern_chrome_like']} path="censorship.tls_fetch.profiles" onChange={() => {}} />);
    expect(tls).toContain('Профили TLS'); expect(tls).toContain('modern_firefox_like'); expect(tls).not.toContain('secret_mode');
    const log = renderToStaticMarkup(<ConfigField value="normal" path="general.log_level" onChange={() => {}} />);
    expect(log).toContain('value="silent"'); expect(log).not.toContain('value="warn"');
  });
});
describe('honest telemetry presentation', () => {
  it('contains out-of-range diagnostic timestamps at the display boundary', () => {
    expect(renderToStaticMarkup(<DataView value={18446744073709551615} path="ts_epoch_secs" />)).toContain('Некорректная дата');
  });
  it('splits unavailable samples and missing collection intervals', () => {
    const samples = [{at:0,values:[1]},{at:5,values:[null]},{at:10,values:[2]},{at:100,values:[3]}];
    expect(segments(samples,0,15).map(part => part.map(row => row.at))).toEqual([[0],[10],[100]]);
  });
  it('scales only rendered series rather than hidden connection counters', () => {
    const svg = renderToStaticMarkup(<Chart title="Скорость" samples={[{at:1000,values:[2,10000]}]} series={['Трафик']} />);
    expect(svg).not.toContain('10 000'); expect(svg).toContain('cy="30"');
  });
  it('never translates account identities or credentials that resemble statuses', () => {
    const html = renderToStaticMarkup(<DataView value={{username:'owner',secret:'normal',role:'owner'}} />);
    expect(html).toContain('>owner<'); expect(html).toContain('>normal<'); expect(html).toContain('>Владелец<');
  });
});

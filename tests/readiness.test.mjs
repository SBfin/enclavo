import assert from 'node:assert/strict';
import { test } from 'node:test';
import { GET } from '../src/app/api/health/ready/route.ts';

test('readiness fails closed on missing configuration and upstream failures, without leaking details', async (t) => {
  const names = ['ENCLAVO_CONTROL_TOKEN', 'ENCLAVO_GATEWAY_URL'];
  const saved = names.map(name => process.env[name]);
  t.after(() => names.forEach((name, i) => {
    if (saved[i] === undefined) delete process.env[name];
    else process.env[name] = saved[i];
  }));
  const request = () => new Request('https://enclavo.example/api/health/ready');
  delete process.env.ENCLAVO_CONTROL_TOKEN;
  const unavailable = await GET(request());
  assert.equal(unavailable.status, 503);
  process.env.ENCLAVO_CONTROL_TOKEN = 'test-control-token';
  process.env.ENCLAVO_GATEWAY_URL = 'http://gateway.internal:8080';
  t.mock.method(globalThis, 'fetch', async (url, init) => {
    assert.equal(url.href, 'http://gateway.internal:8080/readyz');
    assert.equal(init.cache, 'no-store');
    assert.equal(init.headers, undefined);
    return new Response('private database diagnostic', { status: 503 });
  });
  const failed = await GET(request());
  assert.equal(failed.status, 503);
  assert.deepEqual(await failed.json(), { status: 'unavailable' });
  assert.equal(failed.headers.get('cache-control'), 'no-store');
  t.mock.method(globalThis, 'fetch', async () => { throw new Error('private connection string'); });
  assert.deepEqual(await (await GET(request())).json(), { status: 'unavailable' });
  t.mock.method(globalThis, 'fetch', async () => new Response('{"status":"ready"}'));
  const ready = await GET(request());
  assert.equal(ready.status, 200);
  assert.deepEqual(await ready.json(), { status: 'ready' });
});

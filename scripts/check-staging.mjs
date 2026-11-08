// Read-only deployment checks: no credentials, customer writes, or billable inference calls.
import assert from 'node:assert/strict';

const base = new URL(process.argv[2] || 'http://localhost:3002');
assert.ok(['http:', 'https:'].includes(base.protocol), 'Use an HTTP(S) origin');
async function check(path, expected = 200) {
  const response = await fetch(new URL(path, base), { signal: AbortSignal.timeout(15_000) });
  assert.equal(response.status, expected, `${path}: expected ${expected}, received ${response.status}`);
  console.log(`${response.status} ${path}`);
  return response;
}

assert.deepEqual(await (await check('/api/health/ready')).json(), { status: 'ready' });
const home = await check('/');
assert.equal(new URL(home.url).pathname, '/dashboard/market');
const html = await home.text();
const asset = html.match(/(?:src|href)="([^"<>]*\/_next\/static\/[^"<>]+\.(?:js|css))"/);
assert.ok(asset, 'Market must reference a built static asset');
await check(asset[1].replaceAll('&amp;', '&'));
for (const path of ['/dashboard/playground', '/dashboard/api-keys']) await check(path);
const status = await (await check('/api/v1/status')).json();
assert.equal(status.mode, 'live');
assert.ok(status.providers.includes('near-ai'), 'Expected the live NEAR AI provider');
const models = await (await check('/api/v1/models')).json();
assert.ok(models.data?.length > 0, 'Expected a discovered model catalog');
await check('/api/v1/market/auctions');
await check('/api/dashboard/account', 401);
await check('/api/v1/contracts', 401);
await check('/api/v1/internal/account', 404);
console.log('Staging checks passed; no inference requests were made.');

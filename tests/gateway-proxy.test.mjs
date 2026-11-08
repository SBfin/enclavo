import assert from 'node:assert/strict';
import { test } from 'node:test';
import { forwardToGateway } from '../src/lib/gateway-http.ts';
import { readChatStream } from '../src/lib/chat-stream.ts';

const encoder = new TextEncoder();
function sse(text, chunkSize = 7) {
  const bytes = encoder.encode(text);
  return new Response(new ReadableStream({ start(controller) {
    for (let offset = 0; offset < bytes.length; offset += chunkSize) controller.enqueue(bytes.slice(offset, offset + chunkSize));
    controller.close();
  } }), { headers: { 'content-type': 'text/event-stream' } });
}

test('proxy preserves payment challenges and forwards only required credentials', async (t) => {
  t.mock.method(globalThis, 'fetch', async (url, init) => {
    assert.equal(url.href, 'http://gateway:8080/v1/credits/topup?source=client');
    assert.equal(init.headers.get('authorization'), 'Bearer ek_test');
    assert.equal(init.headers.get('payment-signature'), 'signed-payment');
    assert.equal(init.headers.get('cookie'), null);
    assert.equal(init.headers.get('x-forwarded-for'), null);
    assert.equal(await new Response(init.body).text(), '{"amount_usd":"5"}');
    return new Response('{"error":"payment required"}', { status: 402, headers: {
      'payment-required': 'encoded-offer', 'content-type': 'application/json', 'set-cookie': 'private=secret',
    } });
  });
  const response = await forwardToGateway(new Request('http://web/api/v1/credits/topup?source=client', {
    method: 'POST', body: '{"amount_usd":"5"}', headers: {
      authorization: 'Bearer ek_test', 'payment-signature': 'signed-payment', cookie: 'session=private', 'x-forwarded-for': 'forged',
    },
  }), 'http://gateway:8080', '/v1/credits/topup');
  assert.equal(response.status, 402);
  assert.equal(response.headers.get('payment-required'), 'encoded-offer');
  assert.equal(response.headers.get('set-cookie'), null);
  assert.equal(response.headers.get('cache-control'), 'private, no-store');
});

test('proxy returns the first stream chunk before the upstream finishes', async (t) => {
  let finish;
  const body = new ReadableStream({ start(controller) {
    controller.enqueue(encoder.encode('data: {"choices":[]}\n\n'));
    finish = () => { controller.enqueue(encoder.encode('data: [DONE]\n\n')); controller.close(); };
  } });
  t.mock.method(globalThis, 'fetch', async () => new Response(body, { headers: {
    'content-type': 'text/event-stream', 'x-enclavo-route': 'real-route', 'payment-response': 'receipt',
  } }));
  const response = await forwardToGateway(new Request('http://web/api/v1/chat/completions', { method: 'POST', body: '{}' }), 'http://gateway:8080', '/v1/chat/completions');
  assert.equal(response.headers.get('x-enclavo-route'), 'real-route');
  assert.equal(response.headers.get('payment-response'), 'receipt');
  assert.equal(response.headers.get('x-accel-buffering'), 'no');
  const reader = response.body.getReader();
  assert.match(new TextDecoder().decode((await reader.read()).value), /choices/);
  finish();
  assert.match(new TextDecoder().decode((await reader.read()).value), /DONE/);
  reader.releaseLock();
});

test('an unreachable gateway returns an error rather than a fabricated catalog', async (t) => {
  t.mock.method(globalThis, 'fetch', async () => { throw new Error('connection refused'); });
  const response = await forwardToGateway(new Request('http://web/api/v1/models'), 'http://gateway:8080', '/v1/models');
  assert.equal(response.status, 503);
  const data = await response.json();
  assert.equal(data.error.code, 'gateway_unavailable');
  assert.equal(data.data, undefined);
});

test('stream parser preserves split UTF-8 characters, CRLF events, and recorded cost', async () => {
  let text = '';
  const response = sse('data: {"choices":[{"delta":{"content":"Hello 🌍"}}]}\r\n\r\n: enclavo-cost-micro-usd 7\r\n\r\ndata: [DONE]\r\n\r\n', 1);
  const cost = await readChatStream(response, delta => { text += delta; });
  assert.equal(text, 'Hello 🌍');
  assert.equal(cost, 7);
});

test('stream parser rejects truncated and failed responses instead of reporting success', async () => {
  await assert.rejects(readChatStream(sse('data: {"choices":[{"delta":{"content":"partial"}}]}\n\n'), () => {}), /before completion/);
  await assert.rejects(readChatStream(sse('data: {"error":{"message":"provider disconnected"}}\n\n'), () => {}), /provider disconnected/);
});

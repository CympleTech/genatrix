// Design 15, invariants 34 and 36: what the shell does with each request
// the interface sends. Run with `npm test` (Node's own runner, no packages).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { judge, shown } from '../src/lib/gate.ts';

test('approvals are decided only in the shell, on the card it fetches', () => {
  for (const what of ['approve', 'decline', 'edit', 'level']) {
    const v = judge('POST', `/api/action/01ABC/${what}`);
    assert.equal(v.way, 'review');
    assert.equal(v.target, '01ABC');
  }
});

test('devices, pairing, the passphrase, erase and the interface are the shell’s own', () => {
  for (const path of [
    '/api/admin/passphrase', '/api/admin/unlock', '/api/device/d1/admin', '/api/device/d1/revoke',
    '/api/pair/start', '/api/pair', '/api/local/link', '/api/erase', '/api/export',
    '/api/ui/choose', '/api/ui/approve', '/api/ui/copy',
  ]) assert.equal(judge('POST', path).way, 'shell', path);
  assert.equal(judge('GET', '/api/ui').way, 'shell');
});

test('high-privilege and lasting requests are confirmed in the shell first', () => {
  for (const path of [
    '/api/setup/mail', '/api/setup/telegram/start', '/api/setup/lark/start', '/api/setup/remove',
    '/api/agents/install', '/api/agent/a1/uninstall', '/api/work/define', '/api/github/token',
    '/api/github/forget', '/api/person/p1/delete', '/api/group/g1/delete', '/api/person/p1/junk',
    '/api/group/g1/junk', '/api/junk/person/p1/forget', '/api/work/w1/stop', '/api/item/i1/level',
  ]) assert.equal(judge('POST', path).way, 'confirm', path);
});

test('anything not on the list is asked about, never passed', () => {
  assert.equal(judge('POST', '/api/something/new').way, 'confirm');
  assert.equal(judge('DELETE', '/api/item/i1').way, 'confirm');
  assert.equal(judge('POST', '/elsewhere').way, 'shell');
  assert.equal(judge('GET', '/api/../secret').way, 'shell');
});

test('reading and drafting pass', () => {
  assert.equal(judge('GET', '/api/today').way, 'pass');
  assert.equal(judge('GET', '/api/action/01ABC').way, 'pass');
  assert.equal(judge('POST', '/api/work/draft').way, 'pass');
  assert.equal(judge('POST', '/api/item/i1/draft').way, 'pass');
  assert.equal(judge('POST', '/api/ask').way, 'pass');
});

test('what you confirm hides passwords and tokens', () => {
  const fields = Object.fromEntries(shown({ address: 'a@b.c', password: 'hunter2', token: 'ghp_x', nested: { secret: 's', name: 'n' } }));
  assert.equal(fields.address, 'a@b.c');
  assert.equal(fields.password, '••••');
  assert.equal(fields.token, '••••');
  assert.equal(fields['nested.secret'], '••••');
  assert.equal(fields['nested.name'], 'n');
});

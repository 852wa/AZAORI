const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const {test} = require('node:test');

const root = path.resolve(__dirname, '../playground');

function bridge({quota = false, files = {}, userFiles = {}, isolated = true} = {}) {
  const workers = [];
  class Worker {
    constructor() { workers.push(this); }
    postMessage(value) { this.request = value; }
    terminate() { this.terminated = true; }
    emit(data) { this.onmessage({data}); }
  }
  const context = {
    Worker, Response, URL, SharedArrayBuffer, Int32Array, Uint8Array, Atomics,
    TextEncoder, setTimeout, crossOriginIsolated: isolated,
    location: {href: 'http://127.0.0.1:8767/'},
    localStorage: {
      getItem: key => key === 'azaori-rust-files' ? JSON.stringify(userFiles) : null,
      setItem() { if (quota) throw new Error('QuotaExceededError'); },
    },
    fetch: async () => new Response(JSON.stringify({language: {}, samples: [], files})),
  };
  context.window = context;
  vm.createContext(context);
  vm.runInContext(fs.readFileSync(path.join(root, 'wasm-api.js'), 'utf8'), context);
  const request = async (url, body) => (await context.azaoriFetch(url, {body: body ? JSON.stringify(body) : undefined})).json();
  const begin = async source => {
    const pending = request('/api/start', {source});
    await new Promise(resolve => setTimeout(resolve, 0));
    return {worker: workers.at(-1), pending};
  };
  const started = worker => worker.emit({type: 'started', analysis: {tokens: [], tree: null, readings: [], expanded: [], error: null}});
  return {request, begin, started, workers};
}

test('file persistence warnings survive normal completion and pause notes', async () => {
  const api = bridge({quota: true});
  const {worker, pending} = await api.begin('1');
  api.started(worker);
  const {id} = await pending;
  worker.emit({type: 'file', name: 'output.txt', text: 'result\n'});
  worker.emit({type: 'finished', result: {status: 'done', error: null, note: 'Pause limit reached'}});
  const result = await api.request(`/api/poll?id=${id}`);
  assert.match(result.note, /保存容量/);
  assert.match(result.note, /Pause limit reached/);
  assert.equal(result.status, 'done');
});

test('stopping before replacement analysis completes terminates the new worker', async () => {
  const api = bridge();
  const first = await api.begin('1');
  api.started(first.worker);
  await first.pending;
  const replacement = await api.begin('2');
  assert.equal(first.worker.terminated, true);
  await api.request('/api/stop', {id: null});
  assert.equal(replacement.worker.terminated, true);
  assert.equal((await replacement.pending).id, null);
});

test('the UI clears the previous run id before awaiting start', () => {
  const html = fs.readFileSync(path.join(root, 'index.html'), 'utf8');
  const start = html.indexOf('async function run()');
  const end = html.indexOf('start = await', start);
  assert.ok(start >= 0 && end > start);
  assert.match(html.slice(start, end), /currentRun\s*=\s*null\s*;/);
});

test('only the current question can write the shared input buffer', async () => {
  const api = bridge();
  const {worker, pending} = await api.begin('1');
  api.started(worker);
  const {id} = await pending;
  worker.emit({type: 'input', ask: 1, request: {kind: 'text', prompt: 'First', options: []}});
  assert.equal((await api.request('/api/answer', {id, ask: 0, text: 'stale'})).ok, false);
  assert.equal((await api.request('/api/answer', {id, ask: 1, text: 'answer'})).ok, true);
  assert.equal((await api.request('/api/answer', {id, ask: 1, text: 'duplicate'})).ok, false);
  const control = new Int32Array(worker.request.buffer, 0, 4);
  assert.equal(Atomics.load(control, 0), 1);
  assert.equal(Atomics.load(control, 1), 6);
  const text = new TextDecoder().decode(new Uint8Array(worker.request.buffer, 16, 6));
  assert.equal(text, 'answer');
  await api.request('/api/stop', {id});
});

test('prototype property names are ordinary saved program names', async () => {
  const api = bridge();
  const result = await api.request('/api/save', {name: '__proto__', source: '1 足 2'});
  assert.equal(result.ok, true);
  const samples = await api.request('/api/samples');
  assert.deepEqual(samples, [{name: '__proto__', source: '1 足 2'}]);
});

test('bundled and persisted file keys share one normalized namespace', async () => {
  const api = bridge({files: {'./folder//a.txt': 'bundled'}, userFiles: {'folder\\a.txt': 'saved'}});
  const {worker, pending} = await api.begin('1');
  assert.deepEqual(Object.entries(worker.request.files), [['folder/a.txt', 'saved']]);
  await api.request('/api/stop', {id: null});
  await pending;
});

test('virtual files accept prototype names and outside services reject inherited names', async () => {
  const messages = [];
  const context = {
    TextEncoder, TextDecoder, Int32Array, Uint8Array, Atomics,
    init: async () => {},
    analyze: () => JSON.stringify({tree: null, error: null}),
    execute(source, host) {
      assert.equal(host('write_file', JSON.stringify({name: '__proto__', text: 'value\n'})), 'null');
      assert.equal(JSON.parse(host('read_file', JSON.stringify('__proto__'))), 'value\n');
      assert.equal(host('write_file', JSON.stringify({name: './folder//a.txt', text: 'aliased\n'})), 'null');
      assert.equal(JSON.parse(host('read_file', JSON.stringify('folder\\a.txt'))), 'aliased\n');
      assert.match(JSON.parse(host('write_file', JSON.stringify({name: '../escape', text: 'no'}))).host_error, /files/);
      assert.match(JSON.parse(host('outside', JSON.stringify('toString'))).host_error, /知りません/);
      return JSON.stringify({status: 'done', error: null});
    },
    postMessage: value => messages.push(value),
  };
  context.self = context;
  vm.createContext(context);
  const code = fs.readFileSync(path.join(root, 'wasm-worker.js'), 'utf8').replace(/^import .*;\s*/m, '');
  vm.runInContext(code, context);
  await context.onmessage({data: {source: 'test', buffer: new SharedArrayBuffer(32), files: {}}});
  assert.deepEqual(messages.map(message => message.type), ['started', 'file', 'file', 'finished']);
  assert.equal(messages[2].name, 'folder/a.txt');
});

test('editing analysis reuses one worker and answers each request by its number', async () => {
  const api = bridge();
  const first = api.request('/api/analyze', {source: '1 総足 割 個'});
  await new Promise(resolve => setTimeout(resolve, 0));
  const second = api.request('/api/analyze', {source: '2'});
  await new Promise(resolve => setTimeout(resolve, 0));
  assert.equal(api.workers.length, 1);
  const analyzer = api.workers[0];
  while (analyzer.request?.seq !== 2) await new Promise(resolve => setTimeout(resolve, 1));
  assert.deepEqual([analyzer.request.type, analyzer.request.seq, analyzer.request.source], ['analyze', 2, '2']);
  analyzer.emit({type: 'analysis', seq: 2, analysis: {elisions: [], error: null}});
  analyzer.emit({type: 'analysis', seq: 1, analysis: {elisions: [{line: 1, col: 7, end: 8, text: '1'}], error: null}});
  assert.equal((await first).elisions[0].text, '1');
  assert.deepEqual((await second).elisions, []);
  // 実行を始めても、解析用の worker は止めない
  const {worker} = await api.begin('1');
  assert.notEqual(worker, analyzer);
  assert.equal(analyzer.terminated, undefined);
});

test('without cross-origin isolation, programs still run (only input is unavailable)', async () => {
  const api = bridge({isolated: false});
  const {worker, pending} = await api.begin('1 2 3 総足');
  while (!worker.request) await new Promise(resolve => setTimeout(resolve, 1));
  assert.equal(worker.request.buffer, null);
  api.started(worker);
  assert.ok((await pending).id);
});

test('a flood of output is cut at the display limit with a note', async () => {
  const api = bridge();
  const {worker, pending} = await api.begin('1');
  while (!worker.request) await new Promise(resolve => setTimeout(resolve, 1));
  assert.equal(worker.request.maxEvents, 10000);
  api.started(worker);
  const {id} = await pending;
  worker.emit({type: 'finished', result: {status: 'done', error: null, note: null}, dropped: 5});
  const result = await api.request(`/api/poll?id=${id}`);
  assert.match(result.note, /最初の10000件/);
  assert.match(result.note, /10005件/);
});

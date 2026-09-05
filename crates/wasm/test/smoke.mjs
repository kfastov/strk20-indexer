// Run the browser WASM against independently generated native SDK results.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import init, { Engine } from '../pkg/strk20_engine.js';

const file = name => readFileSync(new URL(`../fixture/${name}`, import.meta.url));
const text = name => file(name).toString();
const json = name => JSON.parse(text(name));
const key = owner => Uint8Array.from(Buffer.from(owner.key, 'hex'));
await init({module_or_path:readFileSync(new URL('../pkg/strk20_engine_bg.wasm', import.meta.url))});
const genesis=text('genesis.json'), owners=json('owners.json');
function staged() {
  const engine=new Engine(genesis);
  engine.stage_manifest(text('manifest.json'));
  engine.stage_epoch(0n,file('epochs/0.ndjson'));
  engine.stage_snapshot(0n,file('snapshots/0.zst'),file('snapshots/0.ndjson'));
  engine.stage_head(file('head.ndjson'),'fixture-head');
  engine.stage_checkpoint(text('checkpoint.json'),text('proof.json'));
  return engine;
}
for(const mode of ['auto','epochs']) {
  const engine=staged();
  assert.equal(JSON.parse(engine.apply(mode)).head,99);
  const info=JSON.parse(engine.info());
  assert.equal(info.verified,'rpc-verified');
  assert.equal(info.verifiedAt,99);
  assert.equal(info.snapshot_basis,mode==='auto'?99:null);
  for(const owner of owners) {
    const secret=key(owner);
    assert(secret.some(b=>b!==0));
    const result=JSON.parse(engine.discover(owner.owner,secret));
    assert.deepEqual(result,json(`golden/${mode}/${owner.name}-sdk.json`));
    assert(secret.every(b=>b===0),'zeroize caller key');
    assert(result.notes.length>0,'fixture must exercise real notes');
    for(const note of result.notes) {
      assert(BigInt(note.witness.channelKey)>0n);
      assert.equal(note.knownByBlock,99);
    }
  }
  const bytes=engine.export_state();
  const restored=Engine.load(bytes,genesis);
  // No epoch, snapshot, checkpoint, or apply needed on a trusted local restore.
  assert.deepEqual(JSON.parse(restored.info()),info);
  assert.deepEqual(JSON.parse(restored.discover(owners[0].owner,key(owners[0]))),json(`golden/${mode}/alice-sdk.json`));
  const corrupt=bytes.slice(); corrupt[corrupt.length-33]^=1;
  assert.throws(()=>Engine.load(corrupt,genesis),/STATE_CORRUPT/);
  const foreign=JSON.parse(genesis);foreign.pool='0x999';
  assert.throws(()=>Engine.load(bytes,JSON.stringify(foreign)),/STATE_FOREIGN/);

  const cp=json('checkpoint.json'); cp.state_root='0x123';
  assert.throws(()=>restored.stage_checkpoint(JSON.stringify(cp),text('proof.json')));
  const rejectedKey=key(owners[0]);
  assert.throws(()=>restored.discover(owners[0].owner,rejectedKey),/CHECKPOINT_FAILED/);
  assert(rejectedKey.every(b=>b===0),'zeroize on rejection too');
  const preserved=Engine.load(restored.export_state(),genesis);
  assert.deepEqual(JSON.parse(preserved.info()),info,'failed candidate cannot poison cache');

  restored.stage_checkpoint(text('checkpoint.json'),text('proof.json'));
  restored.stage_head(new TextEncoder().encode('not a head\n'),'corrupt-head');
  assert.throws(()=>restored.apply(mode));
  const preservedAgain=Engine.load(restored.export_state(),genesis);
  assert.deepEqual(JSON.parse(preservedAgain.info()),info,'malformed diff cannot replace verified state');
  engine.free(); restored.free(); preserved.free(); preservedAgain.free();
  console.log(`${mode}: native SDK equality, usable witnesses, zeroization, folded restore, corruption and failed-candidate isolation passed (${bytes.length} cache bytes)`);
}
const fresh=new Engine(genesis);
assert.throws(()=>fresh.apply('auto'),/CHECKPOINT_REQUIRED/);
assert.throws(()=>fresh.export_state(),/CHECKPOINT_REQUIRED/);
fresh.free();
console.log('WASM smoke passed');

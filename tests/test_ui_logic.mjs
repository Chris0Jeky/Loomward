import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
const ctx={};vm.runInNewContext(fs.readFileSync('ui/expansion.js','utf8'),ctx);
const {simulateAdmission,requestFor}=ctx.LoomwardExpansion;
const normalize=x=>JSON.parse(JSON.stringify(x));
const s={capacity:{cpu_slots:1,memory_mib:512},jobs:[{id:'first',demand:{cpu_slots:1,memory_mib:100},priority:9},{id:'other',demand:{cpu_slots:1},priority:0}]};
assert.equal(simulateAdmission(s).accepted[0].job_id,'first');assert.equal(simulateAdmission(s).deferred[0].job_id,'other');
assert.equal(simulateAdmission({...s,telemetry_age_seconds:31}).accepted.length,0);
assert.equal(simulateAdmission({...s,capacity:{cpu_slots:null,memory_mib:512}}).accepted.length,0);
// An omitted capacity dimension is unknown, as in Python scheduler.vector(unknown=True), never 0.
{const r=simulateAdmission({capacity:{cpu_slots:4},jobs:[{id:'j',demand:{memory_mib:100},priority:0}]});assert.equal(r.capacity.memory_mib,null);assert.equal(r.accepted.length,0);assert.ok(JSON.stringify(r).includes('unknown_memory_mib'));}
assert.throws(()=>simulateAdmission({...s,execute:true}));
assert.throws(()=>simulateAdmission({...s,capacity:{cpu_slots:true}}));
assert.throws(()=>requestFor('catalog_search',false));
assert.equal(requestFor('catalog_search',true).params.name,'catalog_search');
assert.equal(requestFor('workspace_summary',false).params._meta['io.modelcontextprotocol/protocolVersion'],'2026-07-28');
assert.throws(()=>requestFor('execute',true));
const fixtures=JSON.parse(fs.readFileSync('fixtures/v2/admission-parity.json','utf8'));
for(const test of fixtures){assert.deepEqual(normalize(simulateAdmission(test.input)),test.expected,test.name);}
console.log(`PASS 9 JS boundary assertions and ${fixtures.length} cross-language admission fixtures`);

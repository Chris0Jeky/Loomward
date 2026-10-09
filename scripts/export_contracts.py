"""Export reference-owned schemas and examples; never connect an MCP host or provider."""
from __future__ import annotations
import json
import sys
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'python'))
from loomward.interop import SCHEMAS, OUTPUT_SCHEMA, obj
from loomward.mcp_stdio import MODERN, LEGACY
DEST=ROOT/'contracts/v2'
DIALECT='https://json-schema.org/draft/2020-12/schema'
TEXT={'type':'string','minLength':1,'maxLength':128}


def main():
    DEST.mkdir(parents=True,exist_ok=True)
    entries={}
    def save(name,data,status='executable_reference',examples=()):
        (DEST/name).write_text(json.dumps(data,indent=2,ensure_ascii=False)+'\n',encoding='utf-8')
        entries[name]={'status':status,'examples':list(examples)}
    for name,schema in SCHEMAS.items():
        save(name+'.input.schema.json',{'$schema':DIALECT,'title':name+' reference input',**schema})
    save('tool-output.schema.json',{'$schema':DIALECT,'title':'Loomward reference result envelope',**OUTPUT_SCHEMA})
    # Design-only contracts are explicit proposals, not installed providers or grants.
    provider=obj({'schema_version':{'const':2},'provider_id':TEXT,'display_name':TEXT,'version':TEXT,
                  'requested_capabilities':{'type':'array','maxItems':16,'uniqueItems':True,'items':{'enum':['observe.summary','observe.metadata','propose.organisation','propose.placement']}},
                  'execution':{'const':'unconfigured'},'installed':{'const':False},
                  'network':{'enum':['none','owner_configured_only']},'settings_schema_version':{'type':'integer','minimum':1}},
                 ('schema_version','provider_id','display_name','version','requested_capabilities','execution','installed','network','settings_schema_version'))
    save('provider-manifest.schema.json',{'$schema':DIALECT,'title':'Draft read/propose provider descriptor, not a grant',**provider},'design_only',['provider-manifest.example.json'])
    save('provider-manifest.example.json',{'schema_version':2,'provider_id':'example.estate-host','display_name':'Example host observation adapter','version':'0.0.0-design',
        'requested_capabilities':['observe.summary'],'execution':'unconfigured','installed':False,'network':'owner_configured_only','settings_schema_version':1},'design_only')
    event=obj({'specversion':{'const':'1.0'},'id':TEXT,'source':{'type':'string','maxLength':256},'type':TEXT,'subject':TEXT,
               'datacontenttype':{'const':'application/json'},'data':obj({'schema_version':{'const':2},'producer_epoch':TEXT,
               'sequence':{'type':'string','pattern':'^(0|[1-9][0-9]*)$','maxLength':20},'scope_ref':TEXT,
               'observation_kind':{'enum':['snapshot_ready','coverage_degraded','proposal_changed']},'origin':{'const':'synthetic_example'}},
               ('schema_version','producer_epoch','sequence','scope_ref','observation_kind','origin'))},
               ('specversion','id','source','type','datacontenttype','data'))
    save('event-envelope.schema.json',{'$schema':DIALECT,'title':'Draft CloudEvents-compatible observation envelope subset',**event},'design_only',['event-envelope.example.json'])
    save('event-envelope.example.json',{'specversion':'1.0','id':'event-demo-001','source':'urn:loomward:demo:catalog','type':'org.loomward.snapshot.ready',
        'subject':'scope-demo','datacontenttype':'application/json','data':{'schema_version':2,'producer_epoch':'demo-epoch','sequence':'1','scope_ref':'scope-demo','observation_kind':'snapshot_ready','origin':'synthetic_example'}},'design_only')
    proposal=obj({'schema_version':{'const':2},'proposal_id':TEXT,'kind':{'enum':['virtual_membership','placement_review','resource_review']},
                  'state':{'enum':['draft','held','needs_evidence','dismissed']},'evidence_refs':{'type':'array','maxItems':100,'items':TEXT},
                  'effect_summary':{'type':'string','maxLength':1024},'source_generation':TEXT,'executable':{'const':False},'approval_grant':{'type':'null'}},
                  ('schema_version','proposal_id','kind','state','evidence_refs','effect_summary','source_generation','executable','approval_grant'))
    save('proposal-record.schema.json',{'$schema':DIALECT,'title':'Draft non-executable review record',**proposal},'design_only',['proposal-record.example.json'])
    save('proposal-record.example.json',{'schema_version':2,'proposal_id':'review-demo-1','kind':'virtual_membership','state':'needs_evidence',
        'evidence_refs':['observation-demo-1'],'effect_summary':'Proposed collection membership; no physical move.', 'source_generation':'demo-generation', 'executable':False,'approval_grant':None},'design_only')
    names=list(SCHEMAS)
    save('reference-capabilities.json',{'schema_version':2,'status':'executable_reference_not_host_certified','transport':'stdio','versions':[MODERN,LEGACY],
       'profiles':{'summary':{'tools':[n for n in names if n not in ('catalog_search','evidence_explain')],'approve':False,'execute':False},
                   'names':{'tools':names,'approve':False,'execute':False}},'resources':['loomward://reference/summary','loomward://reference/policy'],
       'unsupported':['HTTP','sampling','elicitation','tasks','apps','scope_expansion','filesystem_content','provider_connection']})
    meta={'io.modelcontextprotocol/protocolVersion':MODERN,'io.modelcontextprotocol/clientCapabilities':{}}
    def req(i,method,params):return {'jsonrpc':'2.0','id':i,'method':method,'params':dict(params,_meta=meta)}
    save('modern-requests.json',{'origin':'synthetic_examples','requests':[req(1,'server/discover',{}),req(2,'tools/list',{}),req(3,'tools/call',{'name':'workspace_summary','arguments':{}})]})
    save('legacy-requests.json',{'origin':'synthetic_examples','requests':[
       {'jsonrpc':'2.0','id':1,'method':'initialize','params':{'protocolVersion':LEGACY,'capabilities':{},'clientInfo':{'name':'loomward-example','version':'1'}}},
       {'jsonrpc':'2.0','method':'notifications/initialized'}, {'jsonrpc':'2.0','id':2,'method':'tools/list','params':{}}]})
    (DEST/'INDEX.json').write_text(json.dumps({'schema_version':2,'contracts':entries,'note':'Descriptors request authority but never confer it. Example providers are not installed. Output data shape remains tool-specific.'},indent=2)+'\n')
    (DEST/'README.md').write_text('''# Versioned interchange contracts\n\nRun `python scripts/export_contracts.py` from the source checkout to refresh reference-owned schema files. `tests/test_v2_contracts.py` checks schema/runtime parity and validates examples when the optional jsonschema validator is installed. The basic app requires no validator dependency.\n\nThe four input schemas and generic output envelope describe the executable snapshot tool service. Runtime validation adds semantic restrictions: byte strings must fit the reference safe-integer bound, scope cannot expand, forbidden disclosures fail, and placement must satisfy its physics. A schema match alone is not permission.\n\nProvider, event and proposal schemas are **design-only**. There is no provider runtime, event bus or durable proposal store using them yet. Each contains a synthetic example. `INDEX.json` records these status distinctions.\n\nMCP request files are replayable synthetic examples, not host registration files and not proof of compatibility with every client. Pipe them through the process only via the smoke helper or a configured host. The modern and legacy examples use their own lifecycle and metadata.\n''',encoding='utf-8')
    print(f'Exported {len(entries)} versioned contract/example files')
if __name__=='__main__': main()

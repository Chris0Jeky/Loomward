"""Regenerate synthetic demo fixtures. No personal machine data is included."""
from pathlib import Path
import json

ROOT=Path(__file__).resolve().parents[1]
GiB=1024**3
rows=[
 ('Downloads/Invoice-October.pdf',240_000,[]),
 ('Downloads/Conference-recording.mp4',7*GiB,[]),
 ('Downloads/Design-assets.zip',3*GiB,[]),
 ('Downloads/Installer-previous.exe',220_000_000,[]),
 ('Documents/Quarterly-plan.docx',680_000,[]),
 ('Documents/Research-notes.md',32_000,[]),
 ('Projects/Studio/export-master.mov',18*GiB,['project_group']),
 ('Projects/Studio/source-footage.mov',32*GiB,['project_group']),
 ('Projects/App/build-cache.bin',4*GiB,['protected_context']),
 ('Models/Current-assistant.gguf',10*GiB,['pinned']),
 ('Models/Previous-experiment.gguf',14*GiB,[]),
 ('Media/Finished-film.mov',24*GiB,[]),
 ('Media/Finished-film-copy.mov',24*GiB,[]),
 ('Media/Photo-library.zip',8*GiB,[]),
 ('Archive/Completed-project.zip',36*GiB,[]),
 ('Archive/Conference-recording.mp4',7*GiB,[]),
 ('Private/.env',800,['sensitive']),
 ('Desktop/Project-shortcut.lnk',1400,['shortcut']),
]
files=[]
for i,(path,size,flags) in enumerate(rows):
 files.append({'id':f'demo-{i+1:03d}','relative_path':path,'name':path.split('/')[-1],
  'extension':Path(path).suffix.lower(),'size_bytes':size,'allocated_bytes':None,'mtime_ns':0,'ctime_ns':0,
  'device':0,'inode':i+1,'nlink':1,'flags':flags,'identity_quality':'synthetic'})
inv={'schema_version':1,'mode':'demo','root':'Demo workspace','root_key':'synthetic-demo','observed_at':'synthetic fixture, not a scan',
 'files':files,'summary':{'file_count':len(files),'directory_count':10,'logical_bytes':sum(f['size_bytes'] for f in files),
 'allocated_bytes_known_sum':0,'allocated_unknown_count':len(files),'allocation_note':'Synthetic logical sizes; no physical allocation measurement.','elapsed_seconds':0},
 'coverage':{'complete_under_policy':True,'unqualified_complete':True,'limit_hit':False,'examined_entries':len(files),
 'skipped_count':0,'error_count':0,'errors':[],'skipped':[],'max_entries':50000,'max_depth':64},
 'capabilities':{'metadata_scan':False,'content_hash_requires_consent':True,'file_mutation':False,'process_mutation':False}}
volumes=[{'id':'fast','name':'Primary workspace','capacity_bytes':512*GiB,'free_bytes':44*GiB,'reserve_bytes':50*GiB,'tier':0,'online':True,'writable':True},
 {'id':'warm','name':'Project SSD','capacity_bytes':1024*GiB,'free_bytes':310*GiB,'reserve_bytes':100*GiB,'tier':1,'online':True,'writable':True},
 {'id':'archive','name':'Archive HDD','capacity_bytes':2048*GiB,'free_bytes':880*GiB,'reserve_bytes':200*GiB,'tier':2,'online':True,'writable':True}]
groups=[]
for ident,source,dest,heat,pinned,active in [('completed-project',36,36,.03,False,False),('previous-model',14,14,.1,False,False),('current-model',10,10,.8,True,True),('working-film',50,50,.9,False,True)]:
 groups.append({'id':ident,'volume_id':'fast','source_bytes':source*GiB,'destination_bytes':dest*GiB,
 'transfer_bytes':dest*GiB,'heat':heat,'pinned':pinned,'active':active,'protected':False,'days_since_move':90})
scenario={'source_id':'fast','target_free_bytes':90*GiB,'max_transfer_bytes':100*GiB,'cooldown_days':7,'volumes':volumes,'groups':groups}
seed=[]
examples={'Documents':['project notes research summary','meeting notes agenda document','proposal written report'],
 'Finance':['invoice payment receipt','quarterly invoice accounts payment','expense receipt invoice'],
 'Media':['video movie footage','photo image portrait camera','conference recording video'],
 'Projects':['source code repository software','application build software code','design project source'],
 'Models':['assistant gguf model weights','language model experiment gguf','inference model weights'],
 'Archive':['completed project archive old','historical completed archive','previous project archive closed']}
for label,names in examples.items():
 for i,name in enumerate(names):
  seed.append({'event_id':f'seed-{label}-{i}','item_id':f'seed-{label}-{i}','revision':1,'source':'human','label':label,
   'features':{'name':name,'extension':'','context':'','size_bytes':0},'retracted':False})
processes={'status':'demo','sample_kind':'synthetic','total_memory_bytes':32*GiB,'available_memory_bytes':9*GiB,'processes':[
 {'pid':100+i,'start_time':1,'name':n,'resident_bytes':int(g*GiB),'private_commit_bytes':None,'cpu_percent_of_machine':cpu}
 for i,(n,g,cpu) in enumerate([('Model server',10.2,8.5),('Video editor',4.8,11.2),('Browser',2.1,3.1),('Build worker',1.3,19.6),('Loomward reference',.11,.2)])],
 'note':'Synthetic processes. No processes have been inspected or modified.'}
data={'inventory':inv,'volumes':volumes,'scenario':scenario,'seed_events':seed,'processes':processes,
 'duplicate_examples':[{'size_bytes':24*GiB,'logical_duplicate_bytes':24*GiB,'files':[{'id':files[11]['id'],'relative_path':files[11]['relative_path']},{'id':files[12]['id'],'relative_path':files[12]['relative_path']}], 'status':'synthetic_example','deletion_authorised':False}]}
import sys
sys.path.insert(0,str(ROOT/'python'))
from loomward.learning import Student
from loomward.inventory import features_for
model=Student().fit(seed)
data['demo_model']={'model_id':model.model_id,'predictions':{f['id']:model.predict(features_for(f)) for f in files if 'sensitive' not in f['flags']}}
(ROOT/'fixtures/demo.json').write_text(json.dumps(data,indent=2)+'\n',encoding='utf-8')
(ROOT/'ui/demo-data.js').write_text('window.LOOMWARD_DEMO = '+json.dumps(data)+';\n',encoding='utf-8')
print('Generated synthetic demo fixtures.')

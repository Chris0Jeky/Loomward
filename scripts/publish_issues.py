"""Opt-in issue importer. Durable LW IDs prevent repeat publication in normal use.
Does not implement a multi-writer lock: do not run multiple importers concurrently.
"""
import argparse
import json
import re
import shutil
import subprocess
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
def main():
    p=argparse.ArgumentParser();p.add_argument('--repo',required=True);p.add_argument('--execute',action='store_true');p.add_argument('--limit',type=int,default=8);p.add_argument('--all',action='store_true');a=p.parse_args()
    if not re.fullmatch(r'[A-Za-z0-9-]+/[A-Za-z0-9_.-]+',a.repo):raise SystemExit('Use OWNER/REPO')
    issues=json.loads((ROOT/'backlog/issues.json').read_text())
    if not 1<=a.limit<=len(issues):raise SystemExit('Invalid limit')
    selected=issues if a.all else issues[:a.limit]
    if not a.execute:
        print(json.dumps({'dry_run':True,'repository':a.repo,'issues':[{'id':x['id'],'title':x['title']} for x in selected]},indent=2));return 0
    if not shutil.which('gh'):raise SystemExit('GitHub CLI is required')
    # Request limit+1 to detect rather than silently trust truncated coverage.
    r=subprocess.run(['gh','issue','list','--repo',a.repo,'--state','all','--limit','1001','--json','number,body'],check=True,capture_output=True,text=True,timeout=60)
    existing=json.loads(r.stdout)
    if len(existing)>1000:raise SystemExit('Existing issue listing may be incomplete; use an explicit paginated importer')
    bodies='\n'.join(x.get('body','') for x in existing)
    for item in selected:
        marker=f"<!-- loomward:{item['id']} -->"
        if marker in bodies:print('SKIP already present '+item['id']);continue
        body=ROOT/item['body_file']
        subprocess.run(['gh','issue','create','--repo',a.repo,'--title',item['title'],'--body-file',str(body)],check=True,timeout=60)
        print('CREATED '+item['id'])
    return 0
if __name__=='__main__':raise SystemExit(main())

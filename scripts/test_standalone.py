from pathlib import Path
import json
import argparse
from playwright.sync_api import sync_playwright
parser=argparse.ArgumentParser(description='Validate the standalone synthetic preview with an installed Chromium.')
parser.add_argument('--browser',default=None)
parser.add_argument('--output',type=Path)
args=parser.parse_args()
R=Path(__file__).resolve().parents[1]
results=[]
with sync_playwright() as p:
    browser=p.chromium.launch(executable_path=args.browser,headless=True,args=['--no-sandbox'])
    page=browser.new_page(viewport={'width':1440,'height':1000});errors=[];page.on('pageerror',lambda e:errors.append(str(e)))
    page.set_content((R/'preview.html').read_text())
    for name in ['Overview','Storage explorer','Organise & learn','Disk tiers','Processes','Activity','Protection','Decision desk','Connections','Resource budgets']:
        page.get_by_role('button',name=name,exact=True).click()
        assert page.locator('h1').count()==1,name
        results.append({'page':name,'rendered':True,'mode':'standalone_synthetic'})
    page.get_by_role('button',name='Resource budgets',exact=True).click()
    page.locator('#budget-memory').fill('1024');page.locator('[data-action="x-admit"]').click()
    assert 'insufficient_memory_mib' in page.locator('#admission-results').inner_text()
    assert '0 processes changed' in page.locator('#admission-results').inner_text()
    page.get_by_role('button',name='Connections',exact=True).click()
    page.locator('#mcp-tool').select_option('catalog_search');page.locator('[data-action="x-build-request"]').click()
    assert 'Disclosure not enabled' in page.locator('#mcp-request').inner_text()
    page.locator('#mcp-disclose').check();page.locator('[data-action="x-build-request"]').click()
    assert '2026-07-28' in page.locator('#mcp-request').inner_text()
    widths=[]
    for width in (390,768,1440):
        page.set_viewport_size({'width':width,'height':1000})
        for name in ['Decision desk','Connections','Resource budgets']:
            page.get_by_role('button',name=name,exact=True).click()
            assert page.evaluate('document.documentElement.scrollWidth <= innerWidth+1'),(width,name)
        widths.append(width)
    assert not errors,errors
    browser.close()
report={'all_pages_rendered':results,'new_page_widths_checked':widths,'simulation_checked':True,'request_builder_checked':True,'page_errors':errors,'method':'Chromium set_content; standalone synthetic HTML. No provider or local server connected.','windows_webview2_verified':False}
if args.output:
    args.output.parent.mkdir(parents=True,exist_ok=True)
    args.output.write_text(json.dumps(report,indent=2)+'\n')
print('PASS standalone: 10 routes, 3 new routes at 3 widths, simulation and unsent MCP builder; no page errors')

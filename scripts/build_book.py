"""Build a consolidated Markdown reading copy and optional standalone HTML book.

Requires mistune only for HTML. This is a developer packaging helper, not an app
runtime dependency. No network, model calls, external fonts or remote assets.
"""
from __future__ import annotations
import argparse
import html
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]

def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--markdown',type=Path,required=True)
    p.add_argument('--html',type=Path)
    args=p.parse_args()
    paths=sorted((ROOT/'docs').glob('[0-9][0-9]-*.md'))
    chapters=[]
    for source in paths:
        text=source.read_text(encoding='utf-8')
        title=next((x[2:] for x in text.splitlines() if x.startswith('# ')),source.stem)
        ident='chapter-'+source.name[:2]
        note=('> Retained first-pass chapter. Current implementation and evidence are in chapters 23 and 37.\n\n' if int(source.name[:2])<23 else '')
        chapters.append((ident,title,note+text,source.relative_to(ROOT).as_posix()))
    extra=[('sources','Expansion primary-source ledger','docs/EXPANSION-SOURCES.md'),('backlog','100-task implementation backlog','backlog/INDEX.md'),('handoff','Current local-agent launch prompt','handoff/LOCAL-AGENT-PROMPT.md')]
    for ident,title,name in extra:chapters.append((ident,title,(ROOT/name).read_text(encoding='utf-8'),name))
    intro='''# Loomward v0.2\n## Research, architecture, thesis and implementation blueprint\n\n9 October 2026. An expanded, test-backed reference project, not a production Windows optimiser.\n\nThis reading copy consolidates all 41 numbered chapters, including 18 expansion chapters, the new source ledger, the 100-item backlog index and the current local-agent prompt. Detailed issue bodies, source, schemas, experiments and receipts live in the companion source archive. Relative source paths refer to that checkout.\n\n**Current evidence:** 184 Python tests; 14 bridged Chromium checks; nine JavaScript boundary assertions; 80 cross-language admission fixtures; 10 standalone UI pages. Windows/native compilation, real MCP host compatibility, real models and external providers remain unverified. No file/process effect executor is enabled.\n\nChapters 00-22 preserve the first-pass design and evidence. Start at chapter 23 for this expansion, 24 for the thesis, 28 for MCP, 33 for use cases and 37 for current capability limits. The included Git bundle preserves the original base and new expansion branch.\n\n'''
    toc='\n'.join(f'- [{title}](#{ident})' for ident,title,_,_ in chapters)
    markdown=intro+'## Contents\n\n'+toc+'\n\n'
    for ident,title,text,path in chapters:
        markdown+=f'\n---\n\n<a id="{ident}"></a>\n\nSource: `{path}`\n\n{text}\n'
    args.markdown.parent.mkdir(parents=True,exist_ok=True)
    args.markdown.write_text(markdown,encoding='utf-8')
    if args.html:
        try:import mistune
        except ImportError:raise SystemExit('Markdown built; optional mistune is required for HTML. No dependency was installed.')
        render=mistune.create_markdown(escape=True,plugins=['table','task_lists','strikethrough','url'])
        nav=''.join(f'<a href="#{ident}"><span>{ident.replace("chapter-","")}</span>{html.escape(title)}</a>' for ident,title,_,_ in chapters)
        sections=''.join(f'<section id="{ident}" class="chapter"><div class="source">{html.escape(path)}</div>{render(text)}</section>' for ident,_,text,path in chapters)
        css='''*{box-sizing:border-box}html{scroll-behavior:smooth}body{margin:0;color:#203b33;background:#f6f7f1;font:16px/1.7 system-ui,-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif}a{color:#286b50;text-decoration-thickness:1px;text-underline-offset:3px}.sidebar{position:fixed;inset:0 auto 0 0;width:288px;background:#183b31;color:#e4efdb;padding:30px 20px;overflow:auto}.sidebar h2{font-size:24px;margin:0 0 8px}.sidebar p{font-size:13px;color:#b7cebc}.sidebar a{display:block;color:#dfebd6;text-decoration:none;font-size:12px;padding:8px;border-radius:4px;line-height:1.4}.sidebar a:hover{background:#305545}.sidebar a span{display:inline-block;min-width:30px;color:#a9ca96}main{margin-left:288px;padding:50px 6vw 100px;max-width:1430px}.cover{border-bottom:2px solid #acbc9e;padding:20px 0 45px}.eyebrow,.source{font-size:12px;letter-spacing:.08em;color:#718568}.cover h1{font-size:clamp(42px,5vw,78px);letter-spacing:-.06em;line-height:1.1;margin:24px 0}.cover h2{font-weight:450;font-size:27px;max-width:730px;line-height:1.35}.status{padding:18px 22px;border:1px solid #c4d3b6;background:#edf3e6;border-radius:10px;max-width:820px}.metrics{display:flex;gap:35px;flex-wrap:wrap;margin:30px 0}.metrics strong{display:block;font-size:30px;color:#294b37}.metrics span{font-size:13px;color:#63755d}.chapter{padding:48px 0;border-bottom:1px solid #d6dfcd;scroll-margin-top:24px}.chapter h1{font-size:34px;line-height:1.25;letter-spacing:-.025em}.chapter h2{margin-top:34px;font-size:24px;line-height:1.35}.chapter h3{font-size:18px;margin-top:28px}.chapter p,.chapter li{max-width:920px}.chapter table{width:100%;border-collapse:collapse;font-size:13px;line-height:1.55;table-layout:auto;background:#fff;margin:20px 0}.chapter th{text-align:left;background:#e5eedc}.chapter th,.chapter td{border:1px solid #d5dfcc;padding:10px 12px;vertical-align:top;overflow-wrap:anywhere}.chapter pre{background:#183b31;color:#e7efdf;padding:20px;border-radius:8px;overflow:auto;font-size:12px;line-height:1.65}.chapter code{font-family:ui-monospace,Consolas,monospace;font-size:.86em;overflow-wrap:anywhere}.chapter p code,.chapter li code{background:#e7edde;padding:2px 4px;border-radius:3px}.chapter blockquote{margin:20px 0;padding:1px 18px;border-left:4px solid #92ab79;background:#eef2e7;color:#51634b}.chapter img{max-width:100%;height:auto}.mobile-toc{display:none}.footer{font-size:13px;color:#61765b;margin-top:40px}@media(max-width:950px){.sidebar{display:none}main{margin:0;padding:30px 5vw}.mobile-toc{display:block;padding:15px;background:#e6eddc;border-radius:8px}.mobile-toc nav{max-height:60vh;overflow:auto}.mobile-toc a{display:block;padding:6px;font-size:13px}.mobile-toc a span{margin-right:10px}.chapter{overflow-wrap:anywhere}.chapter table{font-size:11px}.chapter td,.chapter th{padding:7px}.cover h2{font-size:23px}}@media print{.sidebar,.mobile-toc{display:none}main{margin:0;max-width:none;padding:0}.chapter{break-before:page}.chapter table{font-size:10px}.chapter pre{white-space:pre-wrap;color:#222;background:#eee}.cover{break-after:page}a{color:#222}}'''
        cover='''<div class="eyebrow">RESEARCH PROTOTYPE · 09 OCTOBER 2026</div><h1>Loomward</h1><h2>A learned workspace companion with explicit evidence, bounded decisions and independent authority.</h2><div class="metrics"><div><strong>41</strong><span>architecture &amp; research chapters</span></div><div><strong>100</strong><span>dependency-linked local tasks</span></div><div><strong>184</strong><span>passing Python reference tests</span></div></div><div class="status"><strong>Working reference, not a Windows release.</strong> Native compilation, real MCP hosts, models and provider connections remain unverified. No file or process effect executor is enabled.</div><p>Includes the original blueprint and this expansion. Start at <a href="#chapter-23">23: expansion overview</a>, <a href="#chapter-24">24: thesis</a>, or <a href="#chapter-37">37: verified capability boundary</a>. Source paths refer to the companion archive. No external assets are required to read this book.</p>'''
        output=f'<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta http-equiv="Content-Security-Policy" content="default-src \'none\'; style-src \'unsafe-inline\'; img-src data:; base-uri \'none\'; form-action \'none\'"><title>Loomward v0.2 | Research and architecture</title><style>{css}</style></head><body><aside class="sidebar"><h2>Loomward</h2><p>v0.2 · Architecture &amp; research book</p><nav>{nav}</nav></aside><main><header class="cover">{cover}</header><details class="mobile-toc"><summary>Contents</summary><nav>{nav}</nav></details>{sections}<p class="footer">The project archive contains full source, issue bodies, contracts, tests and receipts. This book distinguishes design targets from executed reference evidence.</p></main></body></html>'
        args.html.parent.mkdir(parents=True,exist_ok=True);args.html.write_text(output,encoding='utf-8')
    print(f'Built {len(chapters)} reading sections: {len(paths)} numbered chapters plus sources/backlog/handoff.')
if __name__=='__main__':main()

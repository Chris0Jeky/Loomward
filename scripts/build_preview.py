"""Build a self-contained, synthetic-only HTML preview from committed UI assets."""
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
def main():
    page=(ROOT/'ui/index.html').read_text(encoding='utf-8')
    page=page.replace('<link rel="stylesheet" href="/styles.css">','<style>'+(ROOT/'ui/styles.css').read_text(encoding='utf-8')+'</style>')
    page=page.replace('<script src="/demo-data.js" defer></script><script src="/expansion.js" defer></script><script src="/app.js" defer></script>','')
    scripts='\n'.join('<script>'+ (ROOT/'ui'/name).read_text(encoding='utf-8').replace('</script','<\\/script')+'</script>' for name in ['demo-data.js','expansion.js','app.js'])
    policy='<meta http-equiv="Content-Security-Policy" content="default-src \'none\'; script-src \'unsafe-inline\'; style-src \'unsafe-inline\'; img-src data:; connect-src \'none\'; object-src \'none\'; base-uri \'none\'; form-action \'none\'">'
    page=page.replace('<head>','<head>\n'+policy).replace('</body>',scripts+'\n</body>')
    target=ROOT/'preview.html';target.write_text(page,encoding='utf-8')
    print('Built preview.html from local UI assets; synthetic data only, network connections disabled.')
if __name__=='__main__':main()

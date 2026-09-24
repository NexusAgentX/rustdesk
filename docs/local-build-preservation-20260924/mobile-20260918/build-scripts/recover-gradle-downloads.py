from pathlib import Path
from concurrent.futures import ThreadPoolExecutor
from urllib.parse import urlparse
import hashlib, re, subprocess, sys
root=Path('/Users/laysath/Library/Caches/rustdesk-build/mobile-20260918')
urls=set(re.findall(r"Could not GET '(https://[^']+)'",Path(sys.argv[1]).read_text()))
def fetch(url):
 parsed=urlparse(url)
 if parsed.hostname not in ['dl.google.com','repo.maven.apache.org']:raise ValueError('Unexpected repository host')
 path=parsed.path.split('/maven2/',1)[1];parts=path.split('/');group='.'.join(parts[:-3]);artifact,version,name=parts[-3:]
 output=root/'downloads'/name
 subprocess.run(['curl','-fsSL','--retry','5','--retry-all-errors','--connect-timeout','15','--max-time','120',url,'-o',str(output)],check=True)
 checksum=output.with_name(name+'.sha1')
 subprocess.run(['curl','-fsSL','--retry','5','--retry-all-errors','--connect-timeout','15','--max-time','120',url+'.sha1','-o',str(checksum)],check=True)
 actual=hashlib.sha1(output.read_bytes()).hexdigest();expected=checksum.read_text().split()[0]
 if actual!=expected:raise ValueError('Official Maven checksum mismatch: '+name)
 dest=Path.home()/'.gradle/caches/modules-2/files-2.1'/group/artifact/version/actual;dest.mkdir(parents=True,exist_ok=True)
 import shutil
 shutil.copyfile(output,dest/name)
 print('Verified and cached '+name,flush=True)
with ThreadPoolExecutor(max_workers=4) as pool:list(pool.map(fetch,urls))

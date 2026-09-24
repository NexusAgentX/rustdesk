from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import subprocess, plistlib, hashlib, shutil
root=Path('/Users/laysath/Library/Caches/rustdesk-build/mobile-20260918/downloads/ios26-runtime');root.mkdir(exist_ok=True)
metadata=plistlib.loads(Path('/Users/laysath/Library/Caches/rustdesk-build/mobile-20260918/downloads/ios26-runtime-metadata.plist').read_bytes())
asset=next(a for a in metadata['Assets'] if a['Build']=='23C52' and a['Architectures']==['arm64'])
url=asset['__BaseURL']+asset['__RelativePath'];size=asset['_DownloadSize'];step=(size+15)//16
ranges=[(i,start,min(start+step,size)-1) for i,start in enumerate(range(0,size,step))]
def fetch(item):
 i,start,end=item;p=root/f'part-{i}'
 attempts=0
 while not p.exists() or p.stat().st_size<end-start+1:
  offset=start+(p.stat().st_size if p.exists() else 0)
  limit=min(offset+64*1024*1024-1,end)
  temp=root/f'part-{i}.tail';headers=root/f'part-{i}.headers'
  temp.unlink(missing_ok=True)
  headers.unlink(missing_ok=True)
  result=subprocess.run(['curl','-fsSL','--connect-timeout','20','--max-time','300','--speed-time','30','--speed-limit','1024','--range',f'{offset}-{limit}',url,'-D',str(headers),'-o',str(temp)])
  header=headers.read_text(errors='replace').lower() if headers.exists() else ''
  if f'content-range: bytes {offset}-' not in header:
   attempts+=1
   if attempts>20:raise RuntimeError(f'Invalid range header {i} after retries')
   continue
  count=temp.stat().st_size if temp.exists() else 0
  if count>limit-offset+1:raise RuntimeError(f'Oversized range {i}')
  if count:
   with p.open('ab') as dst, temp.open('rb') as src:shutil.copyfileobj(src,dst)
   temp.unlink()
  if result.returncode:
   attempts+=1
   if attempts>20:raise RuntimeError(f'Range retries exhausted {i}')
 if p.stat().st_size!=end-start+1:raise RuntimeError(f'Invalid range size {i}')
 print(f'Part {i} completed',flush=True)
with ThreadPoolExecutor(max_workers=16) as pool:list(pool.map(fetch,ranges))
output=root/'ios-runtime.aar';h=hashlib.sha256()
with output.open('wb') as dst:
 for i,_,_ in ranges:
  with (root/f'part-{i}').open('rb') as src:
   while chunk:=src.read(1024*1024):dst.write(chunk);h.update(chunk)
if h.digest()!=asset['_Measurement-SHA256']:raise RuntimeError('Official asset SHA256 mismatch')
print(f'Official Apple asset SHA256 verified: {h.hexdigest()}',flush=True)
extracted=root/'extracted';extracted.mkdir(exist_ok=True)
decrypted=root/'decrypted.aar'
subprocess.run(['aea','decrypt','-i',str(output),'-o',str(decrypted),'-key-value','base64:'+asset['ArchiveDecryptionKey']],check=True)
empty=root/'empty-source';empty.mkdir(exist_ok=True)
subprocess.run(['aa','patch','-i',str(decrypted),'-src',str(empty),'-dst',str(extracted)],check=True)
if not list(extracted.rglob('*.dmg')):raise RuntimeError('Runtime asset contains no disk image')
print('AppleArchive patch extracted; import the disk image with simctl runtime add.',flush=True)

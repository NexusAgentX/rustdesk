from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import subprocess, plistlib, hashlib, shutil
root=Path('/Users/laysath/Library/Caches/rustdesk-build/mobile-20260918/downloads/android-image-parts');root.mkdir(exist_ok=True)
url='https://dl.google.com/android/repository/sys-img/google_apis/arm64-v8a-35_r09.zip';size=1778933980
prefix_path=Path('/Users/laysath/Library/Android/sdk/.sdk/arch/16f5bceca236b2737008977c4aaf826e46a8de7d.part');prefix=prefix_path.stat().st_size;step=(size-prefix+7)//8
ranges=[(i,start,min(start+step,size)-1) for i,start in enumerate(range(prefix,size,step))]
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
with ThreadPoolExecutor(max_workers=8) as pool:list(pool.map(fetch,ranges))
output=prefix_path.with_suffix('');h=hashlib.sha1()
with output.open('wb') as dst:
 with prefix_path.open('rb') as src:
  while chunk:=src.read(1024*1024):dst.write(chunk);h.update(chunk)
 for i,_,_ in ranges:
  with (root/f'part-{i}').open('rb') as src:
   while chunk:=src.read(1024*1024):dst.write(chunk);h.update(chunk)
if h.hexdigest()!='16f5bceca236b2737008977c4aaf826e46a8de7d':raise RuntimeError('Google image SHA1 mismatch')
print('Official Android image SHA1 verified; populated standard SDK archive cache',flush=True)
subprocess.run(['/Users/laysath/Library/Android/sdk/cmdline-tools/latest/bin/android','--sdk=/Users/laysath/Library/Android/sdk','sdk','--platform=mac_arm64','install','emulator','system-images/android-35/google_apis/arm64-v8a'],check=True)

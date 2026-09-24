from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import subprocess, plistlib, hashlib, shutil
root=Path('/Users/laysath/Library/Caches/rustdesk-build/mobile-20260918/downloads/flutter-ios-release-parts');root.mkdir(exist_ok=True)
url='https://storage.googleapis.com/flutter_infra_release/flutter/a18df97ca57a249df5d8d68cd0820600223ce262/ios-release/artifacts.zip';size=505313793;step=(size+7)//8
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
with ThreadPoolExecutor(max_workers=8) as pool:list(pool.map(fetch,ranges))
output=root.parent/'flutter-ios-release.zip';h=hashlib.md5()
with output.open('wb') as dst:
 for i,_,_ in ranges:
  with (root/f'part-{i}').open('rb') as src:
   while chunk:=src.read(1024*1024):dst.write(chunk);h.update(chunk)
import base64, os
if base64.b64encode(h.digest()).decode()!='19x0v3KTbeOdEyBgV/rhBw==':raise RuntimeError('Official Flutter MD5 mismatch')
print(f'Official Flutter archive MD5 verified: {h.hexdigest()}',flush=True)
sdk=root.parent.parent/'flutter-3.24.5'
engine=sdk/'bin/cache/artifacts/engine'
destination=engine/'ios-release';destination.mkdir(exist_ok=True)
subprocess.run(['unzip','-q',str(output),'-d',str(destination)],check=True)
for variant in ['ios','ios-profile','ios-release']:
 directory=engine/variant
 os.chmod(directory,directory.stat().st_mode|0o555)
 for f in directory.rglob('*'):
  if f.is_file() and (f.stat().st_mode&0o100 or f.name=='flutter_tester'):os.chmod(f,f.stat().st_mode|0o555)
 shutil.copyfile(sdk/'LICENSE',directory/'LICENSE')
(sdk/'bin/cache/ios-sdk.stamp').write_text('a18df97ca57a249df5d8d68cd0820600223ce262')
print('iOS cache populated with all three real engine archives',flush=True)

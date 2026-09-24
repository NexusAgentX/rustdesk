from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import subprocess, hashlib, shutil
root=Path('/Users/laysath/Library/Caches/rustdesk-build/mobile-20260918/downloads')
output=root/'android-ndk-r28c-darwin.zip'
size=952495160
prefix=output.stat().st_size
step=(size-prefix+7)//8
ranges=[(i,start,min(start+step,size)-1) for i,start in enumerate(range(prefix,size,step))]
def fetch(item):
 i,start,end=item
 p=root/f'ndk-part-{i}'
 subprocess.run(['curl','-fsSL','--retry','3','--range',f'{start}-{end}','https://dl.google.com/android/repository/android-ndk-r28c-darwin.zip','-o',str(p)],check=True)
 if p.stat().st_size!=end-start+1: raise RuntimeError(f'Invalid range size {i}')
 print(f'Part {i} completed',flush=True)
with ThreadPoolExecutor(max_workers=8) as pool: list(pool.map(fetch,ranges))
with output.open('ab') as dst:
 for i,_,_ in ranges:
  with (root/f'ndk-part-{i}').open('rb') as src: shutil.copyfileobj(src,dst)
hash=hashlib.sha1(output.read_bytes()).hexdigest()
if hash!='fc20a6bf15a30fb3428c9b60a7308793a362dc6d': raise RuntimeError(f'NDK checksum mismatch {hash}')
print(f'Official repository SHA1 verified: {hash}',flush=True)
subprocess.run(['unzip','-q',str(output),'-d','/Users/laysath/Library/Android/sdk/ndk'],check=True)
Path('/Users/laysath/Library/Android/sdk/ndk/android-ndk-r28c').rename('/Users/laysath/Library/Android/sdk/ndk/28.2.13676358')

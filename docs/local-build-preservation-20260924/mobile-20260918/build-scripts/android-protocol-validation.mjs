import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import net from 'node:net';
const require = createRequire('/Users/laysath/.codex/tmp/rustdesk-mcp-protocol/clients/package.json');
const legacy = require('@modelcontextprotocol/sdk/client/index.js');
const legacyHttp = require('@modelcontextprotocol/sdk/client/streamableHttp.js');
const modern = require('@modelcontextprotocol/client');
const url = new URL(process.env.MCP_TEST_URL ?? 'http://127.0.0.1:37174/mcp');
const authorization = 'Bearer mobile-validation-fixture-not-a-production-credential-20260918';
const root='/Users/laysath/Library/Caches/rustdesk-build/mobile-20260918';
const udid = readFileSync(root+'/ios-simulator-udid.txt','utf8').trim();
const sleep=ms=>new Promise(r=>setTimeout(r,ms));
async function connect(current) {
 const messages=[];
 const Client=current?modern.Client:legacy.Client;
 const Transport=current?modern.StreamableHTTPClientTransport:legacyHttp.StreamableHTTPClientTransport;
 const client=new Client({name:'mobile-validation',version:'1'},current?{capabilities:{},versionNegotiation:{mode:{pin:'2026-07-28'}}}:{capabilities:{}});
 const transport=new Transport(url,{requestInit:{headers:{Authorization:authorization}},fetch:async (input,init)=>{
  if(init?.body)messages.push(JSON.parse(init.body).method);
  return fetch(input,init);
 }});
 await client.connect(transport);
 assert.deepEqual((await client.listTools()).tools.map(t=>t.name),['mobile_validation_fixture']);
 const result=await client.callTool({name:'mobile_validation_fixture',arguments:{action:'status'}});
 assert.equal(result.isError,false);
 const value=JSON.parse(result.content[0].text); assert.equal(value.fixture,true);
 assert(messages.includes(current?'server/discover':'initialize'));
 if(!current)assert(messages.includes('notifications/initialized'));
 return {client,transport,value,messages};
}
for (const current of [false,true]) {
 const test=await connect(current);
 console.log(`${current?'2026-07-28 client2.0.0':'2025-11-25 SDK1.30.0'} discovery/initialize, tools/list and real tools/call passed`,JSON.stringify(test.messages));
 await test.client.close();
}
const test=await connect(true);
const before=test.value.dropped;
const pending=test.client.callTool({name:'mobile_validation_fixture',arguments:{action:'pending'}}).then(()=>({unexpected:true}),error=>({cancelled:true,error:String(error)}));
let started=false;
for(let n=0;n<30;n++) {
 const result=await test.client.callTool({name:'mobile_validation_fixture',arguments:{action:'status'}});
 if(JSON.parse(result.content[0].text).started>before){started=true;break;}
 await sleep(100);
}
assert(started);
execFileSync('/Users/laysath/Library/Android/sdk/platform-tools/adb',['-s','emulator-5554','shell','input','keyevent','KEYCODE_HOME']);
const outcome=await Promise.race([pending,sleep(10000).then(()=>({timeout:true}))]);
assert.equal(outcome.cancelled,true,JSON.stringify(outcome));
let closed=false;
for(let n=0;n<30;n++) {
 try { await fetch(url,{method:'POST',headers:{Authorization:authorization,'Content-Type':'application/json'},body:'{}',signal:AbortSignal.timeout(1000)}); }
 catch {closed=true;break;}
 await sleep(100);
}
assert(closed,'listener must close after background');
await test.client.close();
console.log('Background: active request cancelled and listener closed');
execFileSync('/Users/laysath/Library/Android/sdk/platform-tools/adb',['-s','emulator-5554','shell','am','start','-n','com.carriez.flutter_hbb.mcp.validation/com.carriez.flutter_hbb.MainActivity']);
let resumed;
for(let n=0;n<30;n++) {
 try { resumed=await connect(true);break; } catch {await sleep(200);}
}
assert(resumed,'listener must restart on foreground');
assert(resumed.value.dropped>before,'pending handler must have been dropped before resumed requests');
console.log('Resume: discovery and tools/call passed; prior handler dropped',JSON.stringify(resumed.value));
await resumed.client.close();

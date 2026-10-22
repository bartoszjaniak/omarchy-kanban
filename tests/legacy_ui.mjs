// Browser acceptance test. Requires Node 22+ and Chromium; no provider/model calls.
import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';
import {mkdtemp,readFile,chmod,writeFile} from 'node:fs/promises';
import net from 'node:net';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {setTimeout as sleep} from 'node:timers/promises';

const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..');
const dir=await mkdtemp('/tmp/opencode/kanban-ui-');
await chmod(path.join(root,'tests/fake_opencode.py'),0o755);
const listener=net.createServer();await new Promise(r=>listener.listen(0,'127.0.0.1',r));const port=listener.address().port;await new Promise(r=>listener.close(r));
const core=spawn(process.env.KANBAN_BINARY||path.join(root,'target/debug/omarchy-kanban'),['legacy-serve','--data-dir',path.join(dir,'core'),'--port',String(port),'--opencode',path.join(root,'tests/fake_opencode.py')],{stdio:['ignore','ignore','pipe']});
let browser,ws;let logs='';core.stderr.on('data',b=>logs+=b);
const deadline=setTimeout(()=>{core.kill('SIGKILL');browser?.kill('SIGKILL');console.error('UI test timed out');process.exitCode=1},65000);
async function until(fn){for(let n=0;n<120;n++){try{const value=await fn();if(value)return value}catch{}await sleep(200)}throw new Error('Condition timed out')}
try {
  const token=await until(async()=>await readFile(path.join(dir,'core/token'),'utf8'));
  await until(async()=>(await fetch(`http://127.0.0.1:${port}/`)).ok);
  browser=spawn(process.env.CHROMIUM||'chromium',['--headless','--disable-gpu','--no-first-run','--no-default-browser-check','--remote-debugging-port=0','--user-data-dir='+path.join(dir,'browser'),'about:blank'],{stdio:['ignore','ignore','pipe']});
  const endpoint=await new Promise((resolve,reject)=>{let stderr='';browser.stderr.on('data',b=>{stderr+=b;const m=stderr.match(/DevTools listening on (ws:\/\/[^\s]+)/);if(m)resolve(m[1])});browser.on('error',reject);browser.on('exit',code=>reject(new Error('Chromium exited '+code)))});
  ws=new WebSocket(endpoint);await new Promise((resolve,reject)=>{ws.addEventListener('open',resolve,{once:true});ws.addEventListener('error',reject,{once:true})});
  let next=1;const pending=new Map(),listeners=[],errors=[];
  ws.addEventListener('message',({data})=>{const m=JSON.parse(String(data));if(m.id){const p=pending.get(m.id);pending.delete(m.id);if(m.error)p.reject(m.error);else p.resolve(m.result)}else{if(m.method==='Runtime.exceptionThrown')errors.push(m.params);listeners.forEach(f=>f(m))}});
  const call=(method,params={},sessionId)=>new Promise((resolve,reject)=>{const id=next++;pending.set(id,{resolve,reject});ws.send(JSON.stringify({id,method,params,...(sessionId?{sessionId}:{})}))});
  const {targetId}=await call('Target.createTarget',{url:'about:blank'});const {sessionId}=await call('Target.attachToTarget',{targetId,flatten:true});
  await call('Page.enable',{},sessionId);await call('Runtime.enable',{},sessionId);
  const loaded=new Promise(resolve=>listeners.push(m=>{if(m.method==='Page.loadEventFired'&&m.sessionId===sessionId)resolve()}));
  await call('Page.navigate',{url:`http://127.0.0.1:${port}/`},sessionId);await loaded;
  async function evaluate(expression){const r=await call('Runtime.evaluate',{expression,awaitPromise:true,returnByValue:true},sessionId);assert(!r.exceptionDetails,JSON.stringify(r.exceptionDetails));return r.result.value}
  await until(()=>evaluate('!document.getElementById("login-panel").hidden'));
  await evaluate(`document.getElementById('token').value=${JSON.stringify(token)};document.getElementById('login').requestSubmit();true`);
  await until(()=>evaluate('!document.getElementById("application").hidden'));
  await evaluate(`(() => {document.getElementById('new').click();const f=document.getElementById('task-form');const values={title:'<img src=x onerror=alert(1)>',cwd:${JSON.stringify(dir)},prompt:'TEST_PERMISSION',criteria:'Permission and verified result',agent:'plan'};for(const[k,v]of Object.entries(values))f.elements.namedItem(k).value=v;f.requestSubmit();return true})()`);
  await until(()=>evaluate('document.querySelectorAll(".card").length===1'));
  assert.equal(await evaluate('document.querySelectorAll("img").length'),0);
  await evaluate('document.querySelector("#board .card").click();document.querySelector("#task-actions button:nth-child(2)").click();true');
  await until(()=>evaluate('!!document.querySelector("#decision button")'));
  await evaluate('document.querySelector("#decision button").click();true');
  await until(()=>evaluate('[...document.querySelectorAll("#task-actions button")].some(b=>b.textContent.includes("Akceptuj"))'));
  await evaluate('[...document.querySelectorAll("#task-actions button")].find(b=>b.textContent.includes("Akceptuj")).click();true');
  await until(()=>evaluate('document.getElementById("task-meta").textContent.startsWith("done")'));
  await evaluate(`document.getElementById('new').click();document.getElementById('task-form').elements.namedItem('title').value='Saved browser draft';document.getElementById('save-draft').click();true`);
  assert.equal(await evaluate('document.getElementById("draft-count").textContent'),'1');
  for(const width of [1440,700,390]){await call('Emulation.setDeviceMetricsOverride',{width,height:1000,deviceScaleFactor:1,mobile:false},sessionId);assert(await evaluate('document.documentElement.scrollWidth<=innerWidth'),`Overflow at ${width}`)}
  await call('Emulation.setDeviceMetricsOverride',{width:1440,height:1000,deviceScaleFactor:1,mobile:false},sessionId);
  const screenshot=await call('Page.captureScreenshot',{format:'png'},sessionId);await writeFile(path.join(dir,'ui.png'),Buffer.from(screenshot.data,'base64'));
  assert.equal(errors.length,0,JSON.stringify(errors));
  console.log(JSON.stringify({result:'PASS',scenarios:['login','create task','safe text rendering','ready','ACP permission','review','done','local draft','responsive layout'],widths:[1440,700,390],runtimeErrors:0,screenshot:path.join(dir,'ui.png')}));
}catch(e){console.error(logs);throw e}
finally{clearTimeout(deadline);ws?.close();browser?.kill('SIGTERM');core.kill('SIGINT')}

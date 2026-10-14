import fs from 'node:fs';
import path from 'node:path';
import http from 'node:http';
import {spawn,spawnSync} from 'node:child_process';
// Windows-only, opt-in exact-binary qualification. Run from the repository root.
// Every file belongs to a fresh target directory; no provider home is inherited.
if(process.env.KORU_LIVE_PROVIDER_SMOKE!=='1')throw new Error('explicit_live_guard_required');
if(process.platform!=='win32')throw new Error('platform_not_qualified');
const workspace=process.cwd();
if(!fs.existsSync(path.join(workspace,'Cargo.toml')) || !fs.existsSync(path.join(workspace,'crates/koru-session-runtime/Cargo.toml')))
 throw new Error('run_from_repository_root');
const target=path.join(workspace,'target');
fs.mkdirSync(target,{recursive:true});
const root=fs.mkdtempSync(path.join(target,'grok-child-refusal-'));
const names=['spawn_subagent','task','Task'];
const persistedEnable=process.argv.includes('--persisted-enable');
const result={persistedSubagentsEnabled:persistedEnable,versionExact:false,initialize:false,authenticated:false,session:false,inventory:null,allSessionInventoriesExcludeChildren:true,attempts:[],localRequests:0,unexpectedRequests:0,backgroundRequests:0,proxyRejected:0,nonLoopbackProxyRequests:0,nonJsonLines:0,stderrBytes:0,terminal:'not_started',ownedRootTerminated:false,ownedTreeKillSucceeded:false};
const matches=spawnSync('where.exe',['grok'],{encoding:'utf8',windowsHide:true,timeout:5000}).stdout.trim().split(/\r?\n/);
const exe=matches.find(x=>x.toLowerCase().endsWith('.exe'));
if(!exe){result.terminal='executable_unresolved';console.log(JSON.stringify(result));process.exit(1);}
const version=spawnSync(exe,['--version'],{encoding:'utf8',windowsHide:true,timeout:5000});
result.versionExact=(version.stdout||'').trim()==='grok 1.0.25 (f7e67d6988e2) [stable]';
if(!result.versionExact){result.terminal='version_mismatch';console.log(JSON.stringify(result));process.exit(1);}
const cwd=path.join(root,persistedEnable?'empty-cwd-precedence':'empty-cwd-final'),home=path.join(root,persistedEnable?'isolated-home-precedence':'isolated-home-final');
fs.mkdirSync(cwd,{recursive:true});fs.mkdirSync(home,{recursive:true});
const env={};
for(const k of ['PATH','Path','PATHEXT','SystemRoot','SYSTEMROOT','WINDIR','ComSpec','COMSPEC','TEMP','TMP'])if(process.env[k])env[k]=process.env[k];
Object.assign(env,{GROK_HOME:home,HOME:home,USERPROFILE:home,APPDATA:path.join(home,'appdata'),LOCALAPPDATA:path.join(home,'localappdata'),XAI_API_KEY:'xai-local-fixture-not-a-real-key',GROK_SUBAGENTS:'0',GROK_DISABLE_AUTOUPDATER:'1',GROK_DEFAULT_MODEL:'fixture',NO_COLOR:'1',TEMP:root,TMP:root});
for(const family of ['CURSOR','CLAUDE'])for(const category of ['SKILLS','RULES','AGENTS','MCPS','HOOKS'])env['GROK_'+family+'_'+category+'_ENABLED']='0';
let active=null;
const server=http.createServer(async(req,res)=>{
 result.localRequests++;
 const url=new URL(req.url,'http://127.0.0.1');
 if(!['127.0.0.1','localhost'].includes(url.hostname)){result.nonLoopbackProxyRequests++;res.writeHead(403);res.end('{}');return;}
 let chunks=[],size=0;
 for await(const chunk of req){size+=chunk.length;if(size>2*1024*1024){res.writeHead(413);res.end();return;}chunks.push(chunk);}
 if(req.method==='GET' && url.pathname.endsWith('/models')){res.setHeader('Content-Type','application/json');res.end(JSON.stringify({object:'list',data:[{id:'fixture',object:'model',created:0,owned_by:'local-fixture'}]}));return;}
 if(req.method!=='POST'||!url.pathname.endsWith('/chat/completions')){result.unexpectedRequests++;res.writeHead(404);res.end('{}');return;}
 let body;try{body=JSON.parse(Buffer.concat(chunks).toString());}catch{result.unexpectedRequests++;res.writeHead(400);res.end('{}');return;}
 const offered=(body.tools||[]).map(x=>x.function?.name).filter(Boolean);
 const targeted=!!active&&offered.length>0;
 if(!targeted)result.backgroundRequests++;
 if(targeted){
 active.modelRequests++;
 active.requestInventory={count:offered.length,spawn:offered.includes('spawn_subagent'),task:offered.includes('task'),Task:offered.includes('Task')};
 active.allRequestInventoriesExcludeChildren &&= offered.length>0 && names.every(name=>!offered.includes(name));
 }
 const toolMessages=targeted?(body.messages||[]).filter(x=>x.role==='tool'&&x.tool_call_id==='fixture-call-'+active.name):[];
 if(toolMessages.length){
  const content=toolMessages.map(x=>typeof x.content==='string'?x.content:JSON.stringify(x.content)).join('\n');
  active.toolResultReceived=true;
  active.unknownToolRefusal=/unknown tool|not found|not available|unsupported tool|unrecognized tool|does not exist|no tool/i.test(content)&&content.includes(active.name);
 }
 const first=targeted&&!active.injected;
 if(first)active.injected=true;
 const call={id:'fixture-call-'+(active?.name||'background'),type:'function',function:{name:active?.name||'background',arguments:JSON.stringify({description:'Local refusal fixture',prompt:'Do not run external commands.',subagent_type:'general-purpose'})}};
 const msg=first?{role:'assistant',content:null,tool_calls:[call]}:{role:'assistant',content:'Local fixture complete.'};
 const finish=first?'tool_calls':'stop';
 if(body.stream){
  res.writeHead(200,{'Content-Type':'text/event-stream','Cache-Control':'no-cache'});
  const delta=first?{role:'assistant',tool_calls:[{index:0,...call}]}:{role:'assistant',content:msg.content};
  for(const choice of [{index:0,delta,finish_reason:null},{index:0,delta:{},finish_reason:finish}])res.write('data: '+JSON.stringify({id:'fixture-completion',object:'chat.completion.chunk',created:0,model:'fixture',choices:[choice]})+'\n\n');
  res.end('data: [DONE]\n\n');
 }else{res.setHeader('Content-Type','application/json');res.end(JSON.stringify({id:'fixture-completion',object:'chat.completion',created:0,model:'fixture',choices:[{index:0,message:msg,finish_reason:finish}],usage:{prompt_tokens:1,completion_tokens:1,total_tokens:2}}));}
});
server.on('connect',(_req,socket)=>{result.proxyRejected++;socket.end('HTTP/1.1 403 Forbidden\r\n\r\n');});
await new Promise(r=>server.listen(0,'127.0.0.1',r));
const base='http://127.0.0.1:'+server.address().port+'/v1';
Object.assign(env,{GROK_XAI_API_BASE_URL:base,GROK_MODELS_BASE_URL:base,GROK_MODELS_LIST_URL:base+'/models',HTTP_PROXY:base,HTTPS_PROXY:base,ALL_PROXY:base,NO_PROXY:'127.0.0.1,localhost'});
fs.writeFileSync(path.join(home,'config.toml'),'[models]\ndefault = "fixture"\nmax_retries = 0\n[model.fixture]\nmodel = "fixture"\nname = "Local fixture"\nbase_url = "'+base+'"\nenv_key = "XAI_API_KEY"\napi_backend = "chat_completions"\nmax_retries = 0\ninference_idle_timeout_secs = 5\nsupports_backend_search = false\n'+(persistedEnable?'\n[subagents]\nenabled = true\n':''));
const p=spawn(exe,['agent','stdio'],{cwd,env,windowsHide:true,stdio:['pipe','pipe','pipe']});
const pending=new Map();let next=1,buffer='';
p.stdin.on('error',()=>{});
p.stderr.on('data',c=>{result.stderrBytes+=c.length;});
p.stdout.on('data',c=>{
 buffer+=c.toString();
 if(buffer.length>2*1024*1024){result.terminal='overlong_frame';p.kill();return;}
 for(;;){let end=buffer.indexOf('\n');if(end<0)break;let line=buffer.slice(0,end);buffer=buffer.slice(end+1);
  let f;try{f=JSON.parse(line);}catch{result.nonJsonLines++;continue;}
  if(f.id!==undefined&&pending.has(f.id)){pending.get(f.id)(f);pending.delete(f.id);continue;}
  if(f.method==='session/update'){
   const u=f.params?.update;const ts=u?._meta?.tools;
   if(Array.isArray(ts)){const ns=ts.map(x=>typeof x==='string'?x:x?.name).filter(Boolean);result.inventory={count:ns.length,spawn:ns.includes('spawn_subagent'),task:ns.includes('task'),Task:ns.includes('Task')};result.allSessionInventoriesExcludeChildren &&= ns.length>0&&names.every(name=>!ns.includes(name));}
   if(active&&u?.sessionUpdate==='tool_call_update'&&u.toolCallId==='fixture-call-'+active.name){active.toolUpdates++;if(u.status==='failed')active.failedToolUpdate=true;}
  }
  if(f.id!==undefined&&f.method){p.stdin.write(JSON.stringify({jsonrpc:'2.0',id:f.id,error:{code:-32601,message:'Method not found'}})+'\n');}
 }
});
const rpc=(method,params)=>new Promise((resolve,reject)=>{
 const id=next++,timer=setTimeout(()=>{pending.delete(id);reject(new Error('deadline'));},20000);
 pending.set(id,f=>{clearTimeout(timer);resolve(f);});
 p.stdin.write(JSON.stringify({jsonrpc:'2.0',id,method,params})+'\n');
});
let hard;
try{
 hard=setTimeout(()=>{result.terminal='overall_deadline';if(p.pid)spawnSync('taskkill.exe',['/PID',String(p.pid),'/T','/F'],{windowsHide:true,stdio:'ignore',timeout:5000});},70000);
 const init=await rpc('initialize',{protocolVersion:1,clientInfo:{name:'local-child-refusal-fixture',version:'1'},clientCapabilities:{}});
 result.initialize=!!init.result;
 if(!init.result)throw new Error('initialize_rejected');
 const methods=init.result.authMethods||[];
 const auth=methods.find(x=>x.id==='xai.api_key');
 if(!auth)throw new Error('api_key_method_not_advertised');
 const authenticated=await rpc('authenticate',{methodId:auth.id,_meta:{headless:true}});
 result.authenticated=!!authenticated.result;
 if(authenticated.error){result.authErrorCode=Number.isInteger(authenticated.error.code)?authenticated.error.code:null;throw new Error('authentication_rejected');}
 const session=await rpc('session/new',{cwd,mcpServers:[]});
 result.session=!!session.result?.sessionId;
 if(!result.session){result.sessionErrorCode=Number.isInteger(session.error?.code)?session.error.code:null;throw new Error('session_rejected');}
 for(const name of names){
  active={name,allRequestInventoriesExcludeChildren:true,modelRequests:0,injected:false,toolResultReceived:false,unknownToolRefusal:false,toolUpdates:0,failedToolUpdate:false};
  result.attempts.push(active);
  const answer=await rpc('session/prompt',{sessionId:session.result.sessionId,prompt:[{type:'text',text:'Exercise the local refusal fixture.'}]});
  active.stopReason=['end_turn','cancelled','max_tokens','max_turn_requests','refusal'].includes(answer.result?.stopReason)?answer.result.stopReason:'unknown';
  if(answer.error)active.errorCode=Number.isInteger(answer.error.code)?answer.error.code:null;
  if(active.modelRequests===0)break;
 }
 result.terminal='completed';
}catch(e){result.terminal=['deadline','initialize_rejected','api_key_method_not_advertised','authentication_rejected','session_rejected'].includes(e.message)?e.message:'fixture_error';}
finally{
 clearTimeout(hard);
 if(p.pid){const killed=spawnSync('taskkill.exe',['/PID',String(p.pid),'/T','/F'],{windowsHide:true,stdio:'ignore',timeout:5000});result.ownedTreeKillSucceeded=killed.status===0;}
 await new Promise(r=>setTimeout(r,100));
 result.ownedRootTerminated=p.exitCode!==null;
 for(const resolve of pending.values())resolve({error:{code:-1}});pending.clear();
 server.closeAllConnections();await new Promise(r=>server.close(r));
 result.qualified=result.versionExact&&result.initialize&&result.authenticated&&result.session&&result.allSessionInventoriesExcludeChildren
 &&result.attempts.length===3&&result.attempts.every(a=>a.allRequestInventoriesExcludeChildren
 &&a.toolResultReceived&&a.unknownToolRefusal&&a.failedToolUpdate&&a.stopReason==='end_turn')
 &&result.ownedTreeKillSucceeded&&result.ownedRootTerminated;
 process.exitCode=result.qualified?0:1;
 fs.writeFileSync(path.join(root,persistedEnable?'execution-precedence.sanitized.json':'execution.sanitized.json'),JSON.stringify(result,null,2)+'\n');console.log(JSON.stringify(result,null,2));
}

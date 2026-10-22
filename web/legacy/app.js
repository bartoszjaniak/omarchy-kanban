(() => {
  'use strict';
  const $=id=>document.getElementById(id), el=(tag,text,cls)=>{const node=document.createElement(tag);if(text!==undefined)node.textContent=text;if(cls)node.className=cls;return node;};
  let state=null,online=false,selected=null,decisionId=null,editing=null,busy=false;
  const columns=[['backlog','Backlog'],['todo','TODO'],['running','In Progress'],['waiting','Czeka na Ciebie'],['error','Błąd'],['done','DONE']];
  const storage={get(k){try{return JSON.parse(localStorage.getItem(k)||'null')}catch{return null}},set(k,v){try{localStorage.setItem(k,JSON.stringify(v))}catch{$('error').textContent='Nie można zapisać danych lokalnych. Nie zamykaj szkicu.';throw new Error('localStorage unavailable')}}};
  const key=suffix=>'agent-kanban:'+state.core_id+':'+suffix;
  const actionButton=(text,callback,disabled=false)=>{const b=el('button',text);b.type='button';b.disabled=disabled;b.addEventListener('click',()=>Promise.resolve().then(callback).catch(showError));return b;};
  function showError(e){$('error').textContent=e.message||String(e);}
  async function request(path,body){const response=await fetch(path,{method:body?'POST':'GET',headers:{'content-type':'application/json',...(state?{'x-core-id':state.core_id}:{})},...(body?{body:JSON.stringify(body)}:{})});const text=await response.text();let data;try{data=JSON.parse(text)}catch{data={error:text}}if(!response.ok){const e=new Error(data.error||response.statusText);e.definite=response.status>=400&&response.status<500;throw e}return data;}
  async function command(action,reuse){
    if(!online||!state)throw new Error('Brak połączenia z Core');
    const pending=storage.get(key('pending'));
    if(pending&&!reuse)throw new Error('Najpierw rozstrzygnij niepotwierdzoną komendę');
    if(busy)throw new Error('Poczekaj na odpowiedź Core');
    const body=reuse||{command_id:crypto.randomUUID(),...action};storage.set(key('pending'),body);busy=true;
    try{const result=await request('/api/commands',body);storage.set(key('pending'),null);if(body.action==='add'){const drafts=storage.get(key('drafts'))||[];storage.set(key('drafts'),drafts.filter(d=>d.id!==body.command_id));}$('error').textContent='';await refresh();return result;}
    catch(e){if(e.definite)storage.set(key('pending'),null);throw e}
    finally{busy=false;renderPending();}
  }
  function renderPending(){$('pending-command').hidden=!state||!storage.get(key('pending'));}
  async function refresh(){
    try {
      const next=await request('/api/state');
      if(next.protocol!==1)throw new Error('Niezgodny protokół Core');
      if(state&&next.core_id!==state.core_id)throw new Error('Zmieniła się tożsamość Core — przeładuj po sprawdzeniu hosta');
      state=next;online=true;$('login-panel').hidden=true;$('application').hidden=false;
      if(!selected&&location.hash.startsWith('#task=')){const id=location.hash.slice(6);if(state.tasks.some(t=>t.id===id)){selected=id;$('details').hidden=false;}}
      $('connection').textContent=(state.maintenance?'MAINTENANCE · ':'ONLINE · ')+state.core_id.slice(0,8)+' · '+new Date(state.at*1000).toLocaleTimeString();render();
    }catch(e){online=false;$('connection').textContent='OFFLINE · ostatni odczyt pozostaje w UI';if(!state)$('login-panel').hidden=false;renderDetails();}
  }
  function render(){
    const query=$('search').value.toLowerCase();$('board').replaceChildren();
    for(const [status,label] of columns){const lane=el('div',undefined,'lane');const tasks=state.tasks.filter(t=>(t.status===status||(status==='running'&&t.status==='cancelling'))&&t.title.toLowerCase().includes(query));lane.append(el('h3',label+' · '+tasks.length));for(const task of tasks){const card=el('article',undefined,'card');card.tabIndex=0;card.append(el('small',task.agent+' / próba '+task.attempts+'/'+task.max_attempts),el('p',task.title));if(task.pending)card.append(el('small',task.pending.answering?'ODPOWIEDŹ WYSŁANA':'DECYZJA'));const open=()=>{selected=task.id;decisionId=null;$('details').hidden=false;renderDetails();$('details').scrollIntoView({behavior:'smooth'});};card.addEventListener('click',open);card.addEventListener('keydown',e=>{if(e.key==='Enter')open()});lane.append(card)}$('board').append(lane);}
    renderDetails();renderAutomations();renderDrafts();renderPending();
  }
  function taskAction(action,extra={}){const task=state.tasks.find(t=>t.id===selected);return command({action,id:task.id,revision:task.revision,...extra});}
  function renderDetails(){
    if(!state||!selected)return;
    const task=state.tasks.find(t=>t.id===selected);if(!task)return;
    $('task-heading').textContent=task.title;$('task-meta').textContent=task.status+' · '+task.agent+' · '+task.cwd+' · sesja: '+(task.session_id||'jeszcze nie utworzona');
    $('task-definition').textContent=task.prompt+'\n\nKryteria:\n'+task.criteria;$('result').textContent=task.result||'Brak odpowiedzi';$('task-error').textContent=task.error||'';
    const actions=$('task-actions');actions.replaceChildren();
    if(task.status==='backlog'){actions.append(actionButton('Edytuj',()=>editTask(task),!online),actionButton('Dopuść → TODO',()=>taskAction('ready'),!online));}
    if(task.status==='error')actions.append(actionButton('Ponów w tej sesji — sprawdziłem skutki',()=>taskAction('retry'),!online));
    if(task.status==='waiting'&&task.review_ready)actions.append(actionButton('Akceptuj wynik → DONE',()=>taskAction('accept'),!online));
    if(['todo','error','done','cancelled'].includes(task.status))actions.append(actionButton('Do Backlogu',()=>taskAction('backlog'),!online));
    if(!['done','cancelled','cancelling'].includes(task.status))actions.append(actionButton('Anuluj',()=>taskAction('cancel'),!online));
    $('message-form').hidden=!['done','error','waiting'].includes(task.status)||!!task.pending;
    $('message-form').querySelector('button').disabled=!online;
    if(task.pending){
      const signature=task.pending.id+':'+task.pending.answering+':'+online;
      if(decisionId!==signature){decisionId=signature;renderDecision(task);}
    }else{$('decision').replaceChildren();decisionId=null;}
    const events=state.events.filter(e=>e.task_id===task.id).slice(-30).reverse();$('activity').replaceChildren(...events.map(e=>el('div',new Date(e.at*1000).toLocaleTimeString()+' · '+e.kind+'\n'+JSON.stringify(e.data), 'event')));
  }
  function renderDecision(task){
    const d=task.pending,node=$('decision');node.replaceChildren();node.append(el('h3',d.kind==='permission'?'Wymagana zgoda':'Pytanie agenta'));
    if(d.answering){node.append(el('p','Odpowiedź zapisana. Oczekiwanie na wolny slot wykonawcy lub potwierdzenie ACP.'));return;}
    if(d.kind==='permission'){
      node.append(el('pre',JSON.stringify(d.payload.toolCall,null,2)));
      for(const option of d.payload.options||[])node.append(actionButton(option.name||option.optionId,()=>{
        if(option.kind==='allow_always'&&!confirm('To może zapisać trwałe, szersze uprawnienie w OpenCode. Sprawdziłeś zakres?'))return;
        return taskAction('reply',{decision_id:d.id,response:{option_id:option.optionId}});
      },!online));return;
    }
    node.append(el('p',d.payload.message||'Uzupełnij formularz'));
    const form=el('form');const fields=[];const schema=d.payload.requestedSchema||{};
    for(const [name,property] of Object.entries(schema.properties||{})){
      const label=el('label',property.title||name);let input;
      const choices=property.enum?property.enum.map(v=>({const:v,title:String(v)})):property.oneOf;
      if(choices&&choices.every(c=>'const'in c)){input=el('select');for(const c of choices){const o=el('option',c.title||String(c.const));o.value=JSON.stringify(c.const);input.append(o);}fields.push([name,input,'choice']);}
      else if(property.type==='boolean'){input=el('input');input.type='checkbox';fields.push([name,input,'boolean']);}
      else if(property.type==='array'||property.type==='object'){input=el('textarea');input.placeholder='JSON: '+JSON.stringify(property);fields.push([name,input,'json']);}
      else{input=el('input');input.type=['number','integer'].includes(property.type)?'number':'text';if(property.type==='number')input.step='any';fields.push([name,input,property.type||'string']);}
      input.required=(schema.required||[]).includes(name)&&property.type!=='boolean';input.disabled=!online;label.append(input);form.append(label);
    }
    const submit=el('button','Odpowiedz');submit.disabled=!online;form.append(submit);
    form.addEventListener('submit',async e=>{e.preventDefault();try{const content={};for(const [name,input,type] of fields){if(input.value===''&&!(schema.required||[]).includes(name)&&type!=='boolean')continue;content[name]=type==='boolean'?input.checked:['choice','json'].includes(type)?JSON.parse(input.value):['number','integer'].includes(type)?Number(input.value):input.value;}await taskAction('reply',{decision_id:d.id,response:{action:'accept',content}});}catch(e){showError(e)}});
    node.append(form,actionButton('Odmów odpowiedzi',()=>taskAction('reply',{decision_id:d.id,response:{action:'decline'}}),!online));
  }
  function definition(){const form=$('task-form');const d=Object.fromEntries(new FormData(form));for(const name of ['timeout_secs','max_attempts','priority'])d[name]=Number(d[name]);return {title:d.title,prompt:d.prompt,cwd:d.cwd,agent:d.agent,model:d.model,criteria:d.criteria,timeout_secs:d.timeout_secs,max_attempts:d.max_attempts,priority:d.priority,review_required:true};}
  function editTask(task){editing=task?{id:task.id,revision:task.revision}:null;const form=$('task-form');form.reset();if(task)for(const [k,v]of Object.entries(task)){if(form.elements.namedItem(k))form.elements.namedItem(k).value=v;}form.elements.namedItem('schedule_kind').disabled=!!task;$('editor-title').textContent=task?'Edytuj zadanie':'Nowe zadanie / automatyzacja';$('editor').hidden=false;$('editor').scrollIntoView({behavior:'smooth'});}
  function renderAutomations(){$('automation-list').replaceChildren(...state.automations.map(a=>{const row=el('div',undefined,'card');row.append(el('p',a.template.title+' · '+JSON.stringify(a.schedule)),el('small','Następny termin: '+new Date(a.next_due*1000).toLocaleString()),actionButton(a.enabled?'Wstrzymaj':'Włącz od kolejnego terminu',()=>command({action:'toggle_automation',id:a.id,enabled:!a.enabled}),!online));return row}));}
  function renderDrafts(){const drafts=storage.get(key('drafts'))||[];$('draft-count').textContent=drafts.length;$('draft-list').replaceChildren(...drafts.map(d=>{const row=el('div',undefined,'card');row.append(el('p',d.task.title),actionButton('Otwórz szkic',()=>{editTask(d.task);editing=null;}),actionButton('Wyślij do Backlogu',async()=>{await command({command_id:d.id,action:'add',task:d.task});renderDrafts();},!online),actionButton('Usuń lokalny szkic',()=>{if(confirm('Usunąć tylko ten lokalny szkic?')){storage.set(key('drafts'),drafts.filter(x=>x.id!==d.id));renderDrafts();}}));return row}));}
  $('login').addEventListener('submit',async e=>{e.preventDefault();try{await request('/login',{token:$('token').value});$('token').value='';await refresh();}catch(e){showError(e)}});
  $('new').addEventListener('click',()=>editTask(null));$('close-editor').addEventListener('click',()=>{$('editor').hidden=true});$('close-details').addEventListener('click',()=>{$('details').hidden=true;selected=null;history.replaceState(null,'',location.pathname);});
  $('automations-toggle').addEventListener('click',()=>{$('automations').hidden=!$('automations').hidden});$('drafts-toggle').addEventListener('click',()=>{$('drafts').hidden=!$('drafts').hidden});$('search').addEventListener('input',()=>{if(state)render()});
  $('conversation').addEventListener('click',()=>{editTask(null);const f=$('task-form');f.elements.namedItem('title').value='Rozmowa / retrospektywa';f.elements.namedItem('prompt').value='Przeprowadź retrospektywę pracy na podstawie dostarczonych informacji. Oddziel obserwacje od hipotez. Zaproponuj usprawnienia; nie wdrażaj ich samodzielnie.';f.elements.namedItem('criteria').value='Wnioski ze wskazaniem źródeł, propozycje do zatwierdzenia i sposób sprawdzenia efektu.';});
  $('task-form').addEventListener('submit',async e=>{e.preventDefault();try{if(!online)throw new Error('Offline: zapisz szkic lokalnie');const task=definition(),f=$('task-form'),kind=f.elements.namedItem('schedule_kind').value;const action=editing?{action:'edit',...editing,task}:kind==='none'?{action:'add',task}:{action:'add_automation',template:task,schedule:kind==='daily'?{kind:'daily',time:f.elements.namedItem('time').value,timezone:f.elements.namedItem('timezone').value}:{kind:'interval',minutes:Number(f.elements.namedItem('minutes').value)}};await command(action);$('editor').hidden=true;}catch(e){showError(e)}});
  $('save-draft').addEventListener('click',()=>{try{const drafts=storage.get(key('drafts'))||[];drafts.push({id:crypto.randomUUID(),task:definition()});storage.set(key('drafts'),drafts);renderDrafts();$('editor').hidden=true;$('drafts').hidden=false;}catch(e){showError(e)}});
  $('message-form').addEventListener('submit',async e=>{e.preventDefault();try{await taskAction('message',{text:$('message').value});$('message').value='';}catch(e){showError(e)}});
  $('resolve').addEventListener('click',async()=>{try{await command(null,storage.get(key('pending')));}catch(e){showError(e)}});
  refresh();setInterval(refresh,2000);
})();

(() => {
  'use strict';
  const $=id=>document.getElementById(id);
  const node=(tag,text,cls)=>{const n=document.createElement(tag);if(text!==undefined)n.textContent=text;if(cls)n.className=cls;return n};
  const lanes=[['open','Otwarte'],['running','In Progress'],['waiting','Czeka na Ciebie'],['done','DONE']];
  let state=null,coreOnline=false,selected=null,busy=false,loading=false;
  const storage={get(key){try{return JSON.parse(localStorage.getItem(key)||'null')}catch{return null}},set(key,value){localStorage.setItem(key,JSON.stringify(value))}};
  const pendingKey=()=>`opencode-session-board:${state.core_id}:pending`;
  const report=error=>{$('error').textContent=error.message||String(error)};
  const button=(label,callback,disabled=false)=>{const b=node('button',label);b.type='button';b.disabled=disabled;b.addEventListener('click',()=>Promise.resolve().then(callback).catch(report));return b};
  async function request(path,body){const response=await fetch(path,{method:body?'POST':'GET',headers:{'content-type':'application/json',...(state?{'x-core-id':state.core_id}:{})},...(body?{body:JSON.stringify(body)}:{})});let value;try{value=await response.json()}catch{value={error:'Nieprawidłowa odpowiedź Core'}}if(!response.ok){const e=new Error(value.error||response.statusText);e.status=response.status;throw e}return value}
  function pending(){return state?storage.get(pendingKey()):null}
  async function change(session,action,reuse){
    if(!coreOnline||!state?.connected)throw new Error('Brak aktualnego połączenia z OpenCode');
    if(busy)throw new Error('Poczekaj na potwierdzenie poprzedniej zmiany');
    if(pending()&&!reuse)throw new Error('Najpierw sprawdź niepotwierdzoną komendę');
    const body=reuse||{command_id:crypto.randomUUID(),action,id:session.id,observed_updated:session.updated};
    storage.set(pendingKey(),body);busy=true;
    try{await request('/api/commands',body);storage.set(pendingKey(),null);$('error').textContent='';await refresh()}
    catch(e){if(e.status>=400&&e.status<500)storage.set(pendingKey(),null);throw e}
    finally{busy=false;render()}
  }
  async function refresh(){
    if(loading)return;loading=true;
    try{const next=await request('/api/state');if(next.protocol!==2||next.mode!=='sessions')throw new Error('To nie jest Core tablicy sesji — zaktualizuj usługę');if(state&&next.core_id!==state.core_id)throw new Error('Zmienił się Core — sprawdź połączenie przed przeładowaniem');state=next;coreOnline=true;$('login-panel').hidden=true;$('application').hidden=false;
      if(!selected&&location.hash.startsWith('#session=')){const id=location.hash.slice(9);if(state.sessions.some(s=>s.id===id)){selected=id;$('details').hidden=false}}
      $('connection').textContent=(state.connected?'OPENCODE ONLINE':'OPENCODE OFFLINE')+' · ostatni odczyt: '+(state.last_sync?new Date(state.last_sync*1000).toLocaleTimeString():'—');
      if(state.error)$('error').textContent=state.error;else if($('error').dataset.source==='connection')$('error').textContent='';$('error').dataset.source=state.error?'connection':'';
      $('warnings').textContent=[...state.warnings,...(state.truncated?['Osiągnięto limit 1000 pobranych sesji; aktywne spoza tego zakresu nadal są uwzględniane.']:[])].join(' · ');render();
    }catch(e){coreOnline=false;$('connection').textContent='CORE OFFLINE · ostatnie dane mogą być nieaktualne';if(e.status===401)$('login-panel').hidden=false;report(e);render();}
    finally{loading=false;}
  }
  function open(session){selected=session.id;$('details').hidden=false;renderDetails();$('details').scrollIntoView({behavior:'smooth'})}
  function canMark(session){return coreOnline&&state.connected&&!session.active&&!session.pending_count&&!busy}
  function actions(session){const group=node('div',undefined,'actions');group.append(button('Szczegóły',()=>open(session)));if(session.status==='done')group.append(button('Przywróć → Otwarte',()=>change(session,'session_reopen'),!coreOnline||!state.connected||busy));else{const b=button('✓ DONE',()=>change(session,'session_done'),!canMark(session));b.title=session.active||session.pending_count?'Poczekaj na zakończenie pracy lub rozstrzygnij decyzję w OpenCode':'Oznacz lokalnie jako ukończoną';group.append(b)}return group}
  function render(){
    if(!state)return;
    $('pending-command').hidden=!pending();$('resolve').disabled=!coreOnline||!state.connected||busy;
    const query=$('search').value.toLowerCase(),recent=$('scope').value==='recent',cutoff=Date.now()-7*86400000;
    const sessions=state.sessions.filter(s=>(s.title+' '+s.cwd+' '+s.agent+' '+s.id).toLowerCase().includes(query)&&(!recent||s.active||s.pending_count||s.status==='done'||s.updated>=cutoff));
    $('counts').textContent=`${sessions.length} / ${state.sessions.length} sesji · ${state.sessions.filter(s=>s.active).length} aktywnych`;
    $('board').replaceChildren();
    for(const [status,title]of lanes){const lane=node('section',undefined,'lane');lane.dataset.status=status;const cards=sessions.filter(s=>s.status===status);lane.append(node('h2',title+' · '+cards.length));if(!cards.length)lane.append(node('p','Brak sesji','empty'));
      for(const s of cards){const card=node('article',undefined,'card');card.dataset.id=s.id;const heading=node('h3',s.title);heading.tabIndex=0;heading.addEventListener('click',()=>open(s));heading.addEventListener('keydown',e=>{if(e.key==='Enter')open(s)});card.append(node('div',s.agent+' · '+new Date(s.updated).toLocaleString(),'meta'),heading,node('div',s.cwd,'path'));
        if(s.active_children)card.append(node('p',`Aktywne podsesje: ${s.active_children}`,'badge'));
        if(s.pending_count)card.append(node('p',`Decyzje do podjęcia w OpenCode: ${s.pending_count}`,'badge'));
        if(s.outcome==='failed'||s.outcome==='interrupted')card.append(node('p',s.outcome==='failed'?'Ostatnia tura: błąd':'Ostatnia tura: przerwana','badge'));
        card.append(actions(s));lane.append(card)}$('board').append(lane)}
    renderDetails();
  }
  function renderDetails(){const session=state?.sessions.find(s=>s.id===selected);if(!session){if(selected)$('details').hidden=true;return}$('session-title').textContent=session.title;const fields=[['ID',session.id],['Stan',lanes.find(([s])=>s===session.status)?.[1]||session.status],['Katalog na hoście OpenCode',session.cwd],['Agent',session.agent],['Model',session.model||'—'],['Ostatnia aktywność',new Date(session.updated).toLocaleString()]];$('session-meta').replaceChildren(...fields.flatMap(([k,v])=>[node('dt',k),node('dd',v)]));$('session-actions').replaceChildren(actions(session));$('pending-info').replaceChildren(...session.pending_titles.map(text=>node('p',text)));}
  $('login').addEventListener('submit',async e=>{e.preventDefault();try{await request('/login',{token:$('token').value});$('token').value='';await refresh()}catch(e){report(e)}});
  $('refresh').addEventListener('click',refresh);$('search').addEventListener('input',render);$('scope').addEventListener('change',render);
  $('close-details').addEventListener('click',()=>{selected=null;$('details').hidden=true;history.replaceState(null,'',location.pathname)});
  $('resolve').addEventListener('click',()=>change(null,null,pending()).catch(report));
  refresh();setInterval(refresh,2000);
})();

(() => {
  const nativeFetch=window.fetch.bind(window);
  const dataPromise=nativeFetch('./data.json').then(r=>r.json());
  // wasm は1回だけ読み込んでコンパイルし、実行ごとの worker に渡す（毎回の読み込み直しをしない）
  const wasmModule=(typeof WebAssembly==='object'&&WebAssembly.compileStreaming)
    ? WebAssembly.compileStreaming(nativeFetch('./pkg/azaori_bg.wasm')).catch(()=>null)
    : Promise.resolve(null);
  // 入力（待・認・どれ）を待つには SharedArrayBuffer（HTTPS と COOP/COEP ヘッダー）が要る。ないときは入力だけ使えない
  const canWait=()=>typeof crossOriginIsolated!=='undefined'&&crossOriginIsolated&&typeof SharedArrayBuffer!=='undefined';
  const runs=new Map();
  const MAX_EVENTS=10000;  // 1回の実行で画面に出す結果・途中の値の数
  let nextId=0;
  const fileName=name=>{
    const parts=name.replaceAll('\\','/').split('/');
    return !name||parts[0]===''||name.includes(':')||parts.includes('..') ? null : parts.filter(part=>part&&part!=='.').join('/')||null;
  };
  const fileDictionary=entries=>{
    const files=Object.create(null);
    for(const [name,text] of Object.entries(entries||{})) {
      const key=fileName(name);
      if(key!==null)files[key]=text;
    }
    return files;
  };
  const readLocal=(key,fallback)=>{try{return JSON.parse(localStorage.getItem(key))||fallback;}catch{return fallback;}};
  const saved=Object.assign(Object.create(null),readLocal('azaori-rust-samples',{}));
  const userFiles=fileDictionary(readLocal('azaori-rust-files',{}));
  const response=value=>new Response(JSON.stringify(value),{headers:{'Content-Type':'application/json'}});
  const finish=(run,status,error=null)=>{run.status=status;run.error=error;run.worker?.terminate();run.resolve?.({id:null,tokens:[],tree:null,readings:[],expanded:[],error});run.resolve=null;};

  // 書いている途中の解析は、1つの worker を使い回す（実行用の worker とは別）
  let analyzer=null, analyzeSeq=0;
  const pending=new Map();
  const analyzeSource=source=>new Promise(resolve=>{
    if(!analyzer){
      analyzer=new Worker('./wasm-worker.js',{type:'module'});
      const worker=analyzer;
      analyzer.onmessage=({data:message})=>{
        const done=pending.get(message.seq);
        if(done){pending.delete(message.seq);done(message.analysis);}
        if(message.broken&&analyzer===worker){worker.terminate();analyzer=null;}  // 次の解析では作り直す
      };
      analyzer.onerror=event=>{
        for(const done of pending.values())done({elisions:[],error:{kind:'wasm',message:event.message||'WebAssembly worker error'}});
        pending.clear();analyzer=null;
      };
    }
    const seq=++analyzeSeq, worker=analyzer;
    pending.set(seq,resolve);
    wasmModule.then(module=>worker.postMessage({type:'analyze',seq,source,module}));
  });

  window.azaoriFetch=async (url,options={}) => {
    const path=new URL(url,location.href);
    const body=options.body?JSON.parse(options.body):{};
    const data=await dataPromise;
    switch(path.pathname) {
      case '/api/reference': return response(data.language.reference);
      case '/api/palette': return response(data.language.palette);
      case '/api/samples': {
        const samples=new Map(data.samples.map(s=>[s.name,s]));
        Object.entries(saved).forEach(([name,source])=>samples.set(name,{name,source}));
        return response([...samples.values()].sort((a,b)=>a.name.localeCompare(b.name,'ja')));
      }
      case '/api/save': {
        const name=String(body.name||'').trim();
        if (!name || name.length>60 || /[\\/:*?"<>|]/.test(name)) return response({ok:false,message:'ファイル名に使えない文字があります'});
        try {saved[name]=body.source;localStorage.setItem('azaori-rust-samples',JSON.stringify(saved));return response({ok:true});}
        catch(error){return response({ok:false,message:String(error)});}
      }
      case '/api/analyze': return response(await analyzeSource(String(body.source??'')));
      case '/api/start': {
        for (const run of runs.values()) finish(run,'stopped');
        runs.clear();
        const id=String(++nextId);
        const worker=new Worker('./wasm-worker.js',{type:'module'});
        const buffer=canWait()?new SharedArrayBuffer(16+4000000):null;
        const run={id,worker,buffer,status:'running',events:[],ask:0,error:null};
        runs.set(id,run);
        const start=new Promise(resolve=>run.resolve=resolve);
        worker.onmessage=({data:message})=>{
          switch(message.type) {
            case 'started': {
              const result={...message.analysis,id:message.analysis.error?null:id};
              run.resolve?.(result);run.resolve=null;
              if(result.error)finish(run,'error',result.error);
              break;
            }
            case 'event': run.events.push(message.event);break;
            case 'input': Object.assign(run,{status:'waiting',prompt:message.request.prompt,mode:message.request.kind,options:message.request.options,ask:message.ask});break;
            case 'finished': {
              const dropped=message.dropped?`表示は最初の${MAX_EVENTS}件までです（全部で${MAX_EVENTS+message.dropped}件）`:null;
              const notes=[run.note,message.result.note,dropped].filter(Boolean);
              Object.assign(run,message.result,{note:notes.join('\n')||null});
              run.worker.terminate();break;
            }
            case 'failure': finish(run,'error',message.error);break;
            case 'file': {
              const key=fileName(message.name);
              if(key===null)break;
              userFiles[key]=message.text;
              try{localStorage.setItem('azaori-rust-files',JSON.stringify(userFiles));}catch{run.note='ファイルはこのページを閉じるまで保持します（保存容量の上限）';}
              break;
            }
          }
        };
        worker.onerror=event=>finish(run,'error',{kind:'wasm',message:event.message||'WebAssembly worker error'});
        const files={...fileDictionary(data.files),...userFiles};
        wasmModule.then(module=>worker.postMessage({source:body.source,buffer,files,module,maxEvents:MAX_EVENTS}));
        return response(await start);
      }
      case '/api/poll': {
        await new Promise(resolve=>setTimeout(resolve,25));
        const run=runs.get(path.searchParams.get('id'));
        if(!run)return response({events:[],status:'gone',error:null});
        const {status,prompt,mode,options,ask,error,note,trace,variables}=run;
        return response({events:run.events.slice(Number(path.searchParams.get('since'))||0),status,prompt,mode,options,ask,error,note,trace,variables});
      }
      case '/api/answer': {
        const run=runs.get(body.id);
        if(!run || run.status!=='waiting' || body.ask!==run.ask)return response({ok:false});
        const bytes=new TextEncoder().encode(String(body.text));
        if(bytes.length>4000000){finish(run,'error',{kind:'runtime',message:'入力が長すぎます'});return response({ok:false});}
        new Uint8Array(run.buffer,16).set(bytes);
        const control=new Int32Array(run.buffer,0,4);
        run.status='running';
        Atomics.store(control,1,bytes.length);Atomics.store(control,0,1);Atomics.notify(control,0);
        return response({ok:true});
      }
      case '/api/stop': {for(const run of runs.values())if(!body.id||run.id===body.id)finish(run,'stopped');return response({ok:true});}
      default: return nativeFetch(url,options);
    }
  };
})();

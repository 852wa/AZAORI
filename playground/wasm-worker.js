import init, * as azaori from './pkg/azaori.js';

const { analyze, execute } = azaori;

// 画面側でコンパイルした wasm を受け取って使う（なければ自分で読み込む）
let ready = null;
const start = module => (ready ??= init(module ? {module_or_path: module} : undefined));
// Rust の panic の中身（ブラウザでは「unreachable」としか出ないので、処理系が残した文を見せる）
const describe = error => {
  let detail = '';
  try { detail = azaori.last_panic?.() ?? ''; } catch {}
  return detail ? `処理系の内部エラー: ${detail}` : String(error);
};
const encoder = new TextEncoder();
const decoder = new TextDecoder();
const fileName=name=>{
  const parts=name.replaceAll('\\','/').split('/');
  return !name||parts[0]===''||name.includes(':')||parts.includes('..') ? null : parts.filter(part=>part&&part!=='.').join('/')||null;
};
const textValue = text => ({ shape: [Array.from(text).length], kind: 'text', data: Array.from(text, value => ({type:'char',value})) });
function treeForView(value) {
  if (Array.isArray(value)) return value.map(treeForView);
  if (value && typeof value === 'object') {
    const {type,...fields}=value;
    return type ? {type,fields:Object.fromEntries(Object.entries(fields).map(([k,v])=>[k,treeForView(v)]))} : value;
  }
  return value;
}
self.onmessage = async ({data}) => {
  if (data.type==='analyze') {  // 書いている途中の解析だけ（実行しない。省略の表示などに使う）
    try {
      await start(data.module);
      self.postMessage({type:'analysis',seq:data.seq,analysis:JSON.parse(analyze(data.source))});
    } catch(error) {
      // wasm が途中で止まった（panic）あとは、この worker を使い続けない
      self.postMessage({type:'analysis',seq:data.seq,broken:true,analysis:{elisions:[],error:{kind:'wasm',message:describe(error),line:null}}});
    }
    return;
  }
  try {
    await start(data.module);
    const {source,buffer}=data;
    const maxEvents=data.maxEvents??Infinity;
    let shown=0, dropped=0;
    const control=buffer?new Int32Array(buffer,0,4):null;
    const bytes=buffer?new Uint8Array(buffer,16):null;
    const files=Object.create(null);
    for(const [name,text] of Object.entries(data.files||{})) {
      const key=fileName(name);
      if(key!==null)files[key]=text;
    }
    const analysis=JSON.parse(analyze(source));
    analysis.tree=treeForView(analysis.tree);
    self.postMessage({type:'started',analysis});
    if (analysis.error) return;
    let ask=0;
    const host=(method,json) => {
      const value=JSON.parse(json);
      switch(method) {
        case 'event':
          if (shown<maxEvents) {shown++;self.postMessage({type:'event',event:value});} else dropped++;
          return 'null';
        case 'input': {
          if (!control) return JSON.stringify({host_error:'このページでは入力（待・認・どれ）を使えません（HTTPS と COOP/COEP ヘッダーが必要です）'});
          Atomics.store(control,0,0);
          self.postMessage({type:'input',request:value,ask:++ask});
          while (Atomics.load(control,0)===0) Atomics.wait(control,0,0);
          return JSON.stringify(decoder.decode(bytes.slice(0,Atomics.load(control,1))));
        }
        case 'sleep': {
          if (control) Atomics.wait(control,2,0,value*1000);
          else { const end=performance.now()+value*1000; while (performance.now()<end); }  // 止めるときは worker ごと終わらせる
          return 'null';
        }
        case 'outside': {
          const now=new Date();
          const pad=n=>String(n).padStart(2,'0');
          const services={
            '時刻':()=>textValue(`${pad(now.getHours())}:${pad(now.getMinutes())}:${pad(now.getSeconds())}`),
            '日付':()=>textValue(`${now.getFullYear()}-${pad(now.getMonth()+1)}-${pad(now.getDate())}`),
            '曜日':()=>textValue(['日','月','火','水','木','金','土'][now.getDay()]),
            '秒':()=>({shape:[],kind:'',data:[{type:'float',value:now.getTime()/1000}]})
          };
          return JSON.stringify(Object.hasOwn(services,value) ? services[value]() : {host_error:`外の世界の「${value}」は知りません`});
        }
        case 'read_file': {
          const key=fileName(value);
          return JSON.stringify(key!==null&&Object.hasOwn(files,key) ? files[key] : {host_error:`ファイル "${value}" が files にありません`});
        }
        case 'write_file': {
          const key=fileName(value.name);
          if(key===null)return JSON.stringify({host_error:'ファイルは files フォルダの中だけ使えます'});
          if (encoder.encode(value.text).length>10000000) return JSON.stringify({host_error:'ファイルが大きすぎます（10MBまで）'});
          files[key]=value.text;
          self.postMessage({type:'file',name:key,text:value.text});
          return 'null';
        }
        default: return JSON.stringify({host_error:`Unknown host method: ${method}`});
      }
    };
    const result=JSON.parse(execute(source,host));
    self.postMessage({type:'finished',result,dropped});
  } catch(error) {
    self.postMessage({type:'failure',error:{kind:'wasm',message:describe(error),line:null}});
  }
};

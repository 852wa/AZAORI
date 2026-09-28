import http from 'node:http';
import fs from 'node:fs/promises';
import path from 'node:path';
import {fileURLToPath} from 'node:url';

const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'../playground');
let port=Number(process.argv[2]||8767);
const mime={'.html':'text/html; charset=utf-8','.js':'text/javascript; charset=utf-8','.wasm':'application/wasm','.json':'application/json; charset=utf-8','.png':'image/png','.svg':'image/svg+xml'};
const server=http.createServer(async(req,res)=>{
  res.setHeader('Cross-Origin-Opener-Policy','same-origin');
  res.setHeader('Cross-Origin-Embedder-Policy','require-corp');
  res.setHeader('Cache-Control','no-store');
  if(req.method!=='GET'&&req.method!=='HEAD'){res.writeHead(405);res.end();return;}
  try {
    const url=new URL(req.url,'http://localhost');
    const target=path.resolve(root,'.'+decodeURIComponent(url.pathname==='/'?'/index.html':url.pathname));
    if(!target.startsWith(root+path.sep))throw new Error('Outside root');
    const data=await fs.readFile(target);
    res.setHeader('Content-Type',mime[path.extname(target)]||'application/octet-stream');
    res.writeHead(200);res.end(req.method==='HEAD'?undefined:data);
  } catch {res.writeHead(404);res.end('Not found');}
});
server.on('error',error=>{if(error.code==='EADDRINUSE'&&port<8790){port++;server.listen(port,'127.0.0.1');}else{console.error(error);process.exit(1);}});
server.on('listening',()=>console.log(`AZAORI Rust/WASM: http://127.0.0.1:${port}`));
server.listen(port,'127.0.0.1');

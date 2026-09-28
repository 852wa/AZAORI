const fs=require('node:fs');
const path=require('node:path');
const assert=require('node:assert/strict');
const {chromium}=require('playwright');

const root=path.resolve(__dirname,'..');
const base=process.env.AZAORI_URL||'http://127.0.0.1:8767';
const executablePath=process.env.CHROME_PATH;
const sleep=ms=>new Promise(resolve=>setTimeout(resolve,ms));

async function main() {
  const browser=await chromium.launch({...executablePath&&{executablePath},headless:true});
  const page=await browser.newPage({viewport:{width:1440,height:960}});
  const errors=[];page.on('pageerror',e=>errors.push(String(e)));
  try {
    await page.goto(base);
    await page.waitForFunction(()=>document.querySelector('#status').className==='ok');
    assert(await page.evaluate(()=>crossOriginIsolated));
    await page.locator('#auto').uncheck();
    async function run(source) {
      await page.locator('#source').fill(source);
      await page.locator('#run').click();
      await page.waitForFunction(()=>document.querySelector('#stop').hidden&&['ok','err'].includes(document.querySelector('#status').className));
      return page.locator('#results-list .out').allTextContents();
    }
    assert.deepEqual(await run('1 2 3 総足'),['6']);
    const life=fs.readFileSync(path.join(root,'samples/27_ライフゲーム.azo'),'utf8');
    assert.equal((await run(life)).length,3);
    await page.screenshot({path:path.join(root,'target/playground-desktop.png'),fullPage:true});
    for(const tab of ['readings','expanded','analysis','tokens','tree','reference']) {
      // 結果・書き下し はタブ、ほかは「その他」のメニューの中にある
      if(await page.locator(`.card-head .tab[data-view=${tab}]`).count()) await page.locator(`.card-head .tab[data-view=${tab}]`).click();
      else { await page.locator('#more-tab').click(); await page.locator(`#more-menu [data-view=${tab}]`).click(); }
      assert((await page.locator(`#${tab}`).innerText()).trim().length>0);
    }
    await page.locator('[data-view=results]').click();
    await page.locator('#source').fill('「名前」 待\n「進む？」 認\n「赤」 「青」 どれ\n終\n999');
    await page.locator('#run').click();
    await page.locator('#ask:not([hidden]) #ask-input').waitFor({state:'visible'});
    await page.locator('#ask-input').fill('日本語😀');
    await page.locator('#ask-input').press('Enter');
    await page.locator('#ask-yesno:not([hidden]) button').first().click();
    await page.locator('#ask-choice:not([hidden]) button').nth(1).click();
    await page.waitForFunction(()=>document.querySelector('#status').className==='ok');
    const output=await page.locator('#results-list .out').allTextContents();
    assert(output.includes('日本語😀'));assert(output.includes('青'));assert(!output.includes('999'));
    await page.locator('#source').fill('「待機」 待');
    await page.locator('#run').click();
    await page.locator('#ask:not([hidden])').waitFor({state:'visible'});
    await page.locator('#stop').click();
    await page.waitForFunction(()=>document.querySelector('#status').textContent.includes('止めました'));
    await page.locator('#source').fill('0 回 100000 足 1 置 0.1');
    await page.locator('#run').click();
    await page.locator('#stop').waitFor({state:'visible'});
    await page.locator('#stop').click();
    await page.waitForFunction(()=>document.querySelector('#status').textContent.includes('止めました'));
    assert.deepEqual(await run('「Web保存」 外書 「browser-test.txt」\n「browser-test.txt」 外読'),['Web保存','「Web保存」']);
    assert.deepEqual(await run('「browser-test.txt」 外読'),['「Web保存」']);
    assert.deepEqual(await run('「特殊名」 外書 「__proto__」\n「__proto__」 外読'),['特殊名','「特殊名」']);
    assert.deepEqual(await run('「別表記」 外書 「./dir//alias.txt」\n「dir\\alias.txt」 外読'),['別表記','「別表記」']);
    assert.equal(await page.evaluate(async()=> (await (await window.azaoriFetch('/api/save',{body:JSON.stringify({name:'__proto__',source:'7 足 8'})})).json()).ok),true);
    await page.reload();
    await page.waitForFunction(()=>document.querySelector('#status').className==='ok');
    await page.locator('#auto').uncheck();
    assert.deepEqual(await run('「__proto__」 外読\n「dir/alias.txt」 外読'),['「特殊名」','「別表記」']);
    assert(await page.evaluate(async()=> (await (await window.azaoriFetch('/api/samples')).json()).some(sample=>sample.name==='__proto__'&&sample.source==='7 足 8')));
    await run('1 総');
    assert.equal(await page.locator('#status').getAttribute('class'),'err');
    await run('「😀」 足 【未定義】');
    assert.equal(await page.locator('#highlight .h-err').allTextContents().then(xs=>xs.join('')),'【未定義】');
    // 省略した主語は、補って読む字の上に薄く出る（実行しなくても、書いている途中の解析で出る）
    await page.locator('#source').fill('【点数】は 80 65 92 71\n【点数】 総足 割 個\n【f】とは 説\n  引 1。');
    await page.waitForFunction(()=>document.querySelectorAll('#highlight .ghost').length===2);
    assert.deepEqual(await page.locator('#highlight .ghost').evaluateAll(xs=>xs.map(x=>[x.textContent,x.dataset.ghost])),[['個','【点数】'],['引','これ']]);
    await page.locator('#show-ghost').evaluate(x=>{x.checked=false;x.dispatchEvent(new Event('change'));});
    assert(await page.evaluate(()=>document.body.classList.contains('no-ghost')));
    await page.locator('#show-ghost').evaluate(x=>{x.checked=true;x.dispatchEvent(new Event('change'));});
    await run('1 2 3 総足');
    await page.setViewportSize({width:390,height:844});
    await page.screenshot({path:path.join(root,'target/playground-mobile.png'),fullPage:true});
    assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));
    await page.setViewportSize({width:1440,height:960});
    console.log('Browser interaction checks passed');
    if(process.argv.includes('--interactions-only')) {assert.deepEqual(errors,[]);return;}

    // Preserve large JSON numeric tokens until Rust's arbitrary-precision decoder sees them.
    const manifest=JSON.parse(fs.readFileSync(path.join(root,'conformance/manifest.json'),'utf8'));
    const corpusCases=[];
    const specCases=[];
    for(const suite of manifest.suites) {
      const data=JSON.parse(fs.readFileSync(path.join(root,'conformance',suite.path),'utf8'),(key,value,context)=>typeof value==='number'&&!Number.isSafeInteger(value)&&/^[-]?\d+$/.test(context?.source||'')?context.source:value);
      if(suite.format==='case-corpus') corpusCases.push(...data.cases);
      else if(suite.format==='hand-written-spec') {
        const host={seed:12345,inputs:[],available_inputs:[],files:{},outside:{},stop_immediately:false,random_values:[],random_deals:[],random_shuffles:[]};
        for(const spec of data.cases) specCases.push({id:`spec:${spec.name}`,mode:'run',source:spec.source,host,specOutcome:spec.results?{results:spec.results}:{error:spec.error}});
      }
    }
    corpusCases.push(...specCases);
    await page.evaluate(async()=>{window.testWasm=await import('./pkg/azaori.js');await window.testWasm.default();});
    const failures=[];
    for(let offset=0;offset<corpusCases.length;offset+=250) {
      const cases=corpusCases.slice(offset,offset+250);
      const actual=await page.evaluate(cases=>cases.map(c=>{try{return JSON.parse(window.testWasm.conformance_case(JSON.stringify(c)));}catch(error){return {wasm_trap:String(error)};}}),cases);
      for(let i=0;i<cases.length;i++) {
        const diff=cases[i].specOutcome
          ? cases[i].specOutcome.results
            ? difference(cases[i].specOutcome.results,actual[i].results?.map(result=>result.text))
            : actual[i].error?.message===cases[i].specOutcome.error?null:`error: expected ${cases[i].specOutcome.error}, got ${actual[i].error?.message}`
          : difference(cases[i].expected,actual[i]);
        if(diff)failures.push({id:cases[i].id,mode:cases[i].mode,source:cases[i].source,difference:diff,actual:actual[i]});
      }
      if(offset%5000===0)console.log(`WASM ${Math.min(offset+250,corpusCases.length)}/${corpusCases.length}: ${failures.length} differences`);
    }
    fs.writeFileSync(path.join(root,'target/wasm-failures.json'),JSON.stringify(failures,null,2));
    console.log(`WASM: ${corpusCases.length-failures.length}/${corpusCases.length} passed`);
    assert.equal(failures.length,0,JSON.stringify(failures.slice(0,5)));
    assert.deepEqual(errors,[]);
    fs.writeFileSync(path.join(root,'target/web-verification.json'),JSON.stringify({browser:'Chromium',version:browser.version(),cases:corpusCases.length,failures:0,interactionChecks:true,desktop:[1440,960],mobile:[390,844]},null,2));
  } catch(error) {
    console.error(await page.locator('#status').innerText());
    console.error(await page.locator('#results-list').innerText());
    await page.screenshot({path:path.join(root,'target/playground-failure.png'),fullPage:true});
    throw error;
  } finally {await browser.close();}
}
function difference(a,b,p='$') {
  if(a?.type==='float'&&b?.type==='float') {
    const x=Number(a.value),y=Number(b.value);
    if((Number.isNaN(x)&&Number.isNaN(y))||x===y||Math.abs(x-y)<=1e-12+1e-12*Math.abs(x))return null;
  }
  if(Array.isArray(a)&&Array.isArray(b)) {
    if(a.length!==b.length)return `${p}: length ${a.length} != ${b.length}`;
    for(let i=0;i<a.length;i++){const diff=difference(a[i],b[i],`${p}[${i}]`);if(diff)return diff;}return null;
  }
  if(a&&b&&typeof a==='object'&&typeof b==='object') {
    for(const k of Object.keys(a)){const diff=difference(a[k],b[k]??null,`${p}.${k}`);if(diff)return diff;}return null;
  }
  if(typeof a==='number'&&typeof b==='number'&&Math.abs(a-b)<=1e-12+1e-12*Math.abs(a))return null;
  return a===b?null:`${p}: expected ${JSON.stringify(a)}, got ${JSON.stringify(b)}`;
}
main().catch(error=>{console.error(error);process.exitCode=1;});

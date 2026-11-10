import assert from 'node:assert/strict';
import {createRequire} from 'node:module';
const {chromium}=createRequire(import.meta.url)(process.env.PLAYWRIGHT_MODULE||'playwright');
const browser=await chromium.launch({headless:true,executablePath:process.env.CHROME_EXECUTABLE,args:['--autoplay-policy=no-user-gesture-required']});
try{
 const page=await browser.newPage({viewport:{width:1440,height:960}}),errors=[];page.on('pageerror',error=>errors.push(error.message));
 const stableButtons=async(theme)=>{
  await page.locator('.demo-client.theme-'+theme).waitFor();
  await page.evaluate(async()=>{await Promise.all(document.getAnimations().filter(a=>a.effect?.target?.closest?.('.screen-share')).map(a=>a.finished.catch(()=>{})));await new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)));});
  const contrast=await page.locator('.screen-share-actions button,.screen-viewer footer button').evaluateAll(buttons=>{
   const canvas=document.createElement('canvas');canvas.width=canvas.height=1;const ctx=canvas.getContext('2d');
   const rgba=value=>{ctx.clearRect(0,0,1,1);ctx.fillStyle=value;ctx.fillRect(0,0,1,1);return Array.from(ctx.getImageData(0,0,1,1).data);};
   const mix=(a,b,alpha)=>a.slice(0,3).map((value,i)=>value*alpha+b[i]*(1-alpha));
   const luminance=rgb=>rgb.map(v=>{v/=255;return v<=.04045?v/12.92:((v+.055)/1.055)**2.4;}).reduce((n,v,i)=>n+v*[.2126,.7152,.0722][i],0);
   return buttons.map(button=>{const css=getComputedStyle(button);let parent=button.parentElement,back=[0,0,0,0];while(parent&&back[3]===0){back=rgba(getComputedStyle(parent).backgroundColor);parent=parent.parentElement;}const opacity=Number(css.opacity),bg=rgba(css.backgroundColor),fg=rgba(css.color),base=mix(bg,back,bg[3]/255),background=mix(base,back,opacity),foreground=mix(mix(fg,base,fg[3]/255),back,opacity),a=luminance(foreground),b=luminance(background);return {label:button.textContent.trim(),disabled:button.disabled,danger:button.classList.contains('screen-stop'),color:css.color,background:css.backgroundColor,ratio:(Math.max(a,b)+.05)/(Math.min(a,b)+.05)};});
  });
  for(const button of contrast)assert.ok(button.ratio>=(button.disabled||button.danger?3:4.5),theme+' button lacks readable contrast: '+JSON.stringify(button));
  console.log(theme+' rendered button contrast: '+JSON.stringify(contrast));
 };
 await page.goto(process.argv[2]||'http://127.0.0.1:9198');
 await page.getByRole('button',{name:'屏幕共享',exact:true}).click();
 await page.getByRole('button',{name:'分享屏幕',exact:true}).click();await page.getByRole('button',{name:/选择屏幕或窗口|选择画面并开始/}).click();
 await page.getByRole('button',{name:'隐私暂停',exact:true}).click();await page.getByRole('heading',{name:'共享已暂停'}).waitFor();
 assert.equal(await page.locator('.screen-video video').evaluate(e=>getComputedStyle(e).visibility),'hidden');
 await page.screenshot({path:'/tmp/edenvoice-screen-privacy-publisher.png'});
 await page.getByRole('button',{name:'返回房间',exact:true}).click();await page.locator('.screen-viewer').waitFor({state:'hidden'});
 await page.getByRole('button',{name:'查看画面',exact:true}).click();await page.getByRole('heading',{name:'共享已暂停'}).waitFor();
 await page.getByRole('button',{name:'恢复共享',exact:true}).click();assert.equal(await page.locator('.screen-privacy-placeholder').count(),0);
 await page.getByRole('button',{name:'结束共享',exact:true}).click();
 await page.getByRole('button',{name:'朋友暂停中',exact:true}).click();await page.getByRole('button',{name:'观看',exact:true}).click();
 await page.getByText('分享者暂时离开，回来后会继续共享').waitFor();
 assert.equal(await page.getByRole('button',{name:'恢复共享',exact:true}).count(),0);
 await stableButtons('dark');await page.screenshot({path:'/tmp/edenvoice-screen-privacy-dark.png'});
 await page.getByRole('button',{name:'全屏',exact:true}).click();assert.equal(await page.evaluate(()=>document.fullscreenElement?.classList.contains('screen-video')),true);await page.getByRole('heading',{name:'共享已暂停'}).waitFor();await page.evaluate(()=>document.exitFullscreen());
 await page.getByRole('button',{name:'切换明暗主题',exact:true}).click();await stableButtons('light');await page.screenshot({path:'/tmp/edenvoice-screen-privacy-light.png'});
 await page.setViewportSize({width:900,height:700});await page.waitForTimeout(250);assert.equal(await page.locator('.demo-controls').count(),1);await stableButtons('light');await page.screenshot({path:'/tmp/edenvoice-screen-privacy-compact.png'});
 await page.getByRole('button',{name:'切换朋友暂停 / 恢复',exact:true}).click();assert.equal(await page.locator('.screen-privacy-placeholder').count(),0);
 await page.getByRole('button',{name:'深夜放映室',exact:false}).dblclick();await page.getByRole('button',{name:'屏幕共享',exact:true}).click();
 assert.equal(await page.locator('.screen-privacy-placeholder').count(),0);assert.equal(await page.getByRole('button',{name:'隐私暂停',exact:true}).count(),0);
 assert.deepEqual(errors,[]);console.log('PASS: publisher privacy + resume, persistent tab, paused late viewer, fullscreen, dark/light/compact, room cleanup; no JS errors');
}finally{await browser.close();}
